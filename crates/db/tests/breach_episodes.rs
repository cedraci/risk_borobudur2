use analytics::breach::{LiveEpisode, Proposal, Transition};
use chrono::NaiveDate;
use db::auth::marker::{Configure, Settings, View};
use db::auth::AuthCtx;
use db::repo::{CheckResultRow, NewRun, RunContext};
use std::collections::HashMap;

async fn fresh() -> (db::Db, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    std::mem::forget(dir);
    (dbh, edb)
}

fn run_on(day: u32) -> NewRun {
    NewRun {
        nav_date: NaiveDate::from_ymd_opt(2026, 8, day).unwrap(),
        triggered_by: "import".into(),
        import_id: None,
        actor_user_id: None,
        inputs_complete: true,
        input_notes: serde_json::json!({}),
        results: vec![CheckResultRow {
            check_key: "issuer_10".into(),
            scope_label: "Issuer <= 10% NAV (equities + bonds)".into(),
            limit_value: Some(0.10),
            observed_value: Some(0.106),
            status: "breach".into(),
            detail: serde_json::json!({}),
        }],
    }
}

#[tokio::test]
async fn an_episode_opens_carries_its_proposal_and_closes() {
    let (dbh, edb) = fresh().await;
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let configure = scoped.authorize::<Settings, Configure>(1).unwrap();
    let view = scoped.authorize::<Settings, View>(1).unwrap();

    // Day 7: the breach opens, with a proposal attached.
    let run1 = scoped.record_run(&configure, &run_on(7)).await.unwrap();
    let mut proposals = HashMap::new();
    proposals.insert("issuer_10\u{1f}ACME".to_string(), Proposal {
        classification: Some("passive"),
        reason: "no purchase in ACME since the previous snapshot".into(),
    });
    scoped.apply_transitions(
        &configure,
        &RunContext {
            run_id: run1,
            nav_date: NaiveDate::from_ymd_opt(2026, 8, 7).unwrap(),
            actor_label: "system",
            actor_user_id: None,
        },
        &[Transition::Open { check_key: "issuer_10".into(), subject: "ACME".into(), value: Some(0.106) }],
        &proposals,
    ).await.unwrap();

    let open = scoped.breaches_for(&view, Some("open")).await.unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].subject, "ACME");
    assert_eq!(open[0].proposed_classification.as_deref(), Some("passive"));
    assert_eq!(open[0].classification, "unclassified", "a proposal is not a decision");
    assert_eq!(open[0].closed_nav_date, None);
    let id = open[0].id;

    // The episode is live, so the next run sees it.
    let live = scoped.live_episodes(&view).await.unwrap();
    assert_eq!(live, vec![LiveEpisode {
        id, check_key: "issuer_10".into(), subject: "ACME".into(), peak_value: Some(0.106),
    }]);

    // Day 14: it clears on the data. The state does NOT move.
    let run2 = scoped.record_run(&configure, &run_on(14)).await.unwrap();
    scoped.apply_transitions(
        &configure,
        &RunContext {
            run_id: run2,
            nav_date: NaiveDate::from_ymd_opt(2026, 8, 14).unwrap(),
            actor_label: "system",
            actor_user_id: None,
        },
        &[Transition::Close { id }], &HashMap::new(),
    ).await.unwrap();

    let all = scoped.breaches_for(&view, None).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].closed_nav_date, NaiveDate::from_ymd_opt(2026, 8, 14));
    assert_eq!(all[0].state, "open", "clearing on the data is not sign-off");
    assert!(scoped.live_episodes(&view).await.unwrap().is_empty());

    let events = scoped.breach_events(&view, id).await.unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e.event.as_str()).collect();
    assert_eq!(kinds, vec!["opened", "cleared"]);

    edb.stop().await;
}

#[tokio::test]
async fn a_second_live_episode_for_the_same_subject_is_refused() {
    let (dbh, edb) = fresh().await;
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let configure = scoped.authorize::<Settings, Configure>(1).unwrap();
    let view = scoped.authorize::<Settings, View>(1).unwrap();

    let run1 = scoped.record_run(&configure, &run_on(7)).await.unwrap();
    let open = || Transition::Open {
        check_key: "issuer_10".into(), subject: "ACME".into(), value: Some(0.106),
    };
    scoped.apply_transitions(&configure,
        &RunContext {
            run_id: run1,
            nav_date: NaiveDate::from_ymd_opt(2026, 8, 7).unwrap(),
            actor_label: "system",
            actor_user_id: None,
        }, &[open()], &HashMap::new()).await.unwrap();

    let run2 = scoped.record_run(&configure, &run_on(8)).await.unwrap();
    let again = scoped.apply_transitions(&configure,
        &RunContext {
            run_id: run2,
            nav_date: NaiveDate::from_ymd_opt(2026, 8, 8).unwrap(),
            actor_label: "system",
            actor_user_id: None,
        }, &[open()], &HashMap::new()).await;
    let err = again.unwrap_err().to_string();
    assert!(err.contains("idx_breaches_live"),
        "the refusal must come from the partial unique index, not from something else: {err}");
    assert_eq!(scoped.breaches_for(&view, None).await.unwrap().len(), 1,
        "the refused transaction left nothing behind");

    edb.stop().await;
}

#[tokio::test]
async fn raising_the_peak_records_the_worst_reading_and_its_date() {
    let (dbh, edb) = fresh().await;
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let configure = scoped.authorize::<Settings, Configure>(1).unwrap();
    let view = scoped.authorize::<Settings, View>(1).unwrap();

    let run1 = scoped.record_run(&configure, &run_on(7)).await.unwrap();
    scoped.apply_transitions(&configure,
        &RunContext {
            run_id: run1,
            nav_date: NaiveDate::from_ymd_opt(2026, 8, 7).unwrap(),
            actor_label: "system",
            actor_user_id: None,
        },
        &[Transition::Open { check_key: "issuer_10".into(), subject: "ACME".into(), value: Some(0.106) }],
        &HashMap::new()).await.unwrap();
    let id = scoped.breaches_for(&view, None).await.unwrap()[0].id;

    let run2 = scoped.record_run(&configure, &run_on(14)).await.unwrap();
    scoped.apply_transitions(&configure,
        &RunContext {
            run_id: run2,
            nav_date: NaiveDate::from_ymd_opt(2026, 8, 14).unwrap(),
            actor_label: "system",
            actor_user_id: None,
        }, &[Transition::RaisePeak { id, value: 0.131 }], &HashMap::new()).await.unwrap();

    let row = &scoped.breaches_for(&view, None).await.unwrap()[0];
    assert_eq!(row.peak_value, Some(0.131));
    assert_eq!(row.opened_value, Some(0.106), "the opening value is not overwritten");
    assert_eq!(row.peak_nav_date, NaiveDate::from_ymd_opt(2026, 8, 14),
        "the peak carries the date it was struck on, not the opening date");

    edb.stop().await;
}

#[tokio::test]
async fn a_transition_naming_another_portfolios_episode_is_refused() {
    let (dbh, edb) = fresh().await;
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let c1 = scoped.authorize::<Settings, Configure>(1).unwrap();
    let v1 = scoped.authorize::<Settings, View>(1).unwrap();

    // Portfolio 1 has a live episode.
    let run1 = scoped.record_run(&c1, &run_on(7)).await.unwrap();
    scoped.apply_transitions(&c1,
        &RunContext {
            run_id: run1,
            nav_date: NaiveDate::from_ymd_opt(2026, 8, 7).unwrap(),
            actor_label: "system",
            actor_user_id: None,
        },
        &[Transition::Open { check_key: "issuer_10".into(), subject: "ACME".into(), value: Some(0.106) }],
        &HashMap::new()).await.unwrap();
    let victim = scoped.breaches_for(&v1, None).await.unwrap()[0].id;

    // A second fund, and a grant that reaches only it.
    let p2: i64 = sqlx::query_scalar(
        "INSERT INTO portfolios (name, kind) VALUES ('Other','ucits') RETURNING id")
        .fetch_one(dbh.test_pool()).await.unwrap();
    let c2 = scoped.authorize::<Settings, Configure>(p2).unwrap();
    let run2: i64 = sqlx::query_scalar(
        "INSERT INTO limit_check_runs (portfolio_id, nav_date, triggered_by, inputs_complete, input_notes)
         VALUES ($1, DATE '2026-08-08', 'manual', true, '{}'::jsonb) RETURNING id")
        .bind(p2).fetch_one(dbh.test_pool()).await.unwrap();

    let out = scoped.apply_transitions(&c2,
        &RunContext {
            run_id: run2,
            nav_date: NaiveDate::from_ymd_opt(2026, 8, 8).unwrap(),
            actor_label: "system",
            actor_user_id: None,
        }, &[Transition::Close { id: victim }], &HashMap::new()).await;
    assert!(out.is_err(), "a grant on one fund must not close another fund's episode");

    // Portfolio 1's record is untouched: still live, still one event.
    let row = &scoped.breaches_for(&v1, None).await.unwrap()[0];
    assert_eq!(row.closed_nav_date, None, "the victim episode must still be live");
    let events = scoped.breach_events(&v1, victim).await.unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e.event.as_str()).collect();
    assert_eq!(kinds, vec!["opened"], "no falsified 'cleared' event was appended");

    edb.stop().await;
}

#[tokio::test]
async fn a_cleared_but_unsigned_episode_does_not_block_a_new_one() {
    let (dbh, edb) = fresh().await;
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let configure = scoped.authorize::<Settings, Configure>(1).unwrap();
    let view = scoped.authorize::<Settings, View>(1).unwrap();
    let open = || Transition::Open {
        check_key: "issuer_10".into(), subject: "ACME".into(), value: Some(0.106),
    };
    let at = |run_id: i64, d: u32| RunContext {
        run_id, nav_date: NaiveDate::from_ymd_opt(2026, 8, d).unwrap(),
        actor_label: "system", actor_user_id: None,
    };

    let run1 = scoped.record_run(&configure, &run_on(7)).await.unwrap();
    scoped.apply_transitions(&configure, &at(run1, 7), &[open()], &HashMap::new())
        .await.unwrap();
    let first = scoped.breaches_for(&view, None).await.unwrap()[0].id;

    // It clears on the data. Nobody signs it off: state is still `open`.
    let run2 = scoped.record_run(&configure, &run_on(14)).await.unwrap();
    scoped.apply_transitions(&configure, &at(run2, 14),
        &[Transition::Close { id: first }], &HashMap::new()).await.unwrap();

    // The same subject breaches again. This is a second episode, not a revival.
    let run3 = scoped.record_run(&configure, &run_on(21)).await.unwrap();
    scoped.apply_transitions(&configure, &at(run3, 21), &[open()], &HashMap::new())
        .await.unwrap();

    let all = scoped.breaches_for(&view, None).await.unwrap();
    assert_eq!(all.len(), 2, "the cleared-but-unsigned episode must not absorb the new one");
    let still_open: Vec<i64> = all.iter().filter(|b| b.closed_nav_date.is_none()).map(|b| b.id).collect();
    assert_eq!(still_open.len(), 1, "exactly one episode is live");
    assert_ne!(still_open[0], first, "the live one is the new episode, not the cleared one");

    edb.stop().await;
}
