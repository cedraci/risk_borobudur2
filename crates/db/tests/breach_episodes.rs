use analytics::breach::{Finding, LiveEpisode, Proposal, Transition};
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

// ---- The one-transaction entry point (I2, I3, C2, M5) --------------------

fn finding() -> Finding {
    Finding { check_key: "issuer_10".into(), subject: "ACME".into(), value: Some(0.106) }
}

fn no_proposals(_: &[Transition]) -> HashMap<String, Proposal> { HashMap::new() }

/// I2: `record_run` and `apply_transitions` used to be two transactions with
/// three awaits between them. A failure in the second left a committed run
/// whose results say `status = "breach"` beside a register holding no episode
/// — the register contradicting its own run history, and permanently
/// understating how long the fund was in breach once a later run finally
/// opened the episode.
///
/// The failure is induced through the `proposed_classification` CHECK
/// constraint (only `active`/`passive`), which is a real constraint on a real
/// column rather than a fault injected into the plumbing.
#[tokio::test]
async fn a_failing_transition_rolls_back_the_run_that_produced_it() {
    let (dbh, edb) = fresh().await;
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let configure = scoped.authorize::<Settings, Configure>(1).unwrap();
    let view = scoped.authorize::<Settings, View>(1).unwrap();

    let bad = |ts: &[Transition]| {
        let mut m = HashMap::new();
        for t in ts {
            if let Transition::Open { check_key, subject, .. } = t {
                m.insert(format!("{check_key}\u{1f}{subject}"), Proposal {
                    classification: Some("neither"), reason: "violates the CHECK".into(),
                });
            }
        }
        m
    };
    let out = scoped.record_run_and_transitions(
        &configure, &run_on(7), &[finding()], "system", &bad).await;
    assert!(out.is_err(), "the transition must fail, or this test proves nothing");

    assert!(scoped.runs_for(&view, 50).await.unwrap().is_empty(),
        "a run whose transitions failed must not be committed on its own — that is a \
         register that contradicts its own run history");
    assert!(scoped.breaches_for(&view, None).await.unwrap().is_empty());

    edb.stop().await;
}

/// I3: two runs for one portfolio at the same moment (two operators, or one
/// operator and two tabs). Both used to read `live_episodes` on the pool, both
/// computed `Open`, and the loser hit `idx_breaches_live` and rolled back
/// EVERY transition it had computed plus — after I2 — its whole run. With the
/// read and the write inside one transaction under
/// `pg_advisory_xact_lock`, the second run sees the episode the first opened
/// and simply records no transition for it.
#[tokio::test]
async fn two_concurrent_runs_on_one_portfolio_do_not_lose_a_run() {
    let (dbh, edb) = fresh().await;
    let ctx = AuthCtx::desktop();

    let one = async {
        let scoped = dbh.scope(&ctx);
        let c = scoped.authorize::<Settings, Configure>(1).unwrap();
        scoped.record_run_and_transitions(&c, &run_on(7), &[finding()], "system", &no_proposals).await
    };
    let two = async {
        let scoped = dbh.scope(&ctx);
        let c = scoped.authorize::<Settings, Configure>(1).unwrap();
        scoped.record_run_and_transitions(&c, &run_on(7), &[finding()], "system", &no_proposals).await
    };
    let (a, b) = tokio::join!(one, two);
    a.expect("the first run must commit");
    b.expect("the second run must commit too — a lost run is a lost audit record");

    let scoped = dbh.scope(&ctx);
    let view = scoped.authorize::<Settings, View>(1).unwrap();
    assert_eq!(scoped.runs_for(&view, 50).await.unwrap().len(), 2,
        "both runs are real history and both must be recorded");
    let all = scoped.breaches_for(&view, None).await.unwrap();
    assert_eq!(all.len(), 1, "the same subject breaching is ONE episode, not two: {all:?}");

    edb.stop().await;
}

/// C2, at the layer that enforces it: a run older than the newest already
/// recorded records its results and leaves the episode lifecycle alone, and
/// says so in `input_notes`.
#[tokio::test]
async fn a_back_dated_run_records_its_results_and_skips_the_transitions() {
    let (dbh, edb) = fresh().await;
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let configure = scoped.authorize::<Settings, Configure>(1).unwrap();
    let view = scoped.authorize::<Settings, View>(1).unwrap();

    scoped.record_run_and_transitions(&configure, &run_on(14), &[finding()], "system", &no_proposals)
        .await.unwrap();
    let live = scoped.live_episodes(&view).await.unwrap();
    assert_eq!(live.len(), 1, "day 14 opens the episode");

    // Day 7 arrives late, and finds nothing: without the guard, `transitions`
    // would emit `Close` for the day-14 episode and stamp it cleared a week
    // before it opened.
    let out = scoped.record_run_and_transitions(&configure, &run_on(7), &[], "system", &no_proposals)
        .await.unwrap();
    assert_eq!(out.transitions_skipped_after, NaiveDate::from_ymd_opt(2026, 8, 14));

    let all = scoped.breaches_for(&view, None).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].closed_nav_date, None, "the episode must not be closed by a back-dated run");
    assert_eq!(scoped.breach_events(&view, all[0].id).await.unwrap().len(), 1,
        "only the `opened` event — no falsified `cleared`");

    let runs = scoped.runs_for(&view, 50).await.unwrap();
    assert_eq!(runs.len(), 2, "the back-dated run is still recorded: the register is complete");
    let (back, back_results) = runs.iter()
        .find(|(r, _)| r.nav_date == NaiveDate::from_ymd_opt(2026, 8, 7).unwrap()).unwrap();
    assert_eq!(back_results.len(), 1,
        "the back-dated run's RESULTS are honest history for that date and are kept");
    assert!(back.input_notes[db::repo::TRANSITIONS_SKIPPED_NOTE].is_string(),
        "the skip must be stated, never left for a reader to infer: {}", back.input_notes);

    edb.stop().await;
}

/// M5: `Transition::Close` was the one state transition in this module with no
/// guard on the episode's current state, so applying it twice overwrote the
/// close and appended a SECOND `cleared` event to a timeline whose entire
/// value is being exact. A re-application must be a no-op.
#[tokio::test]
async fn closing_an_already_closed_episode_is_a_no_op() {
    let (dbh, edb) = fresh().await;
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let configure = scoped.authorize::<Settings, Configure>(1).unwrap();
    let view = scoped.authorize::<Settings, View>(1).unwrap();

    let run1 = scoped.record_run(&configure, &run_on(7)).await.unwrap();
    scoped.apply_transitions(&configure,
        &RunContext { run_id: run1, nav_date: NaiveDate::from_ymd_opt(2026, 8, 7).unwrap(),
                      actor_label: "system", actor_user_id: None },
        &[Transition::Open { check_key: "issuer_10".into(), subject: "ACME".into(), value: Some(0.106) }],
        &HashMap::new()).await.unwrap();
    let id = scoped.breaches_for(&view, None).await.unwrap()[0].id;

    let run2 = scoped.record_run(&configure, &run_on(14)).await.unwrap();
    scoped.apply_transitions(&configure,
        &RunContext { run_id: run2, nav_date: NaiveDate::from_ymd_opt(2026, 8, 14).unwrap(),
                      actor_label: "system", actor_user_id: None },
        &[Transition::Close { id }], &HashMap::new()).await.unwrap();

    // The same close again, from a later run. It must not move the close date
    // and must not append a second `cleared`.
    let run3 = scoped.record_run(&configure, &run_on(21)).await.unwrap();
    scoped.apply_transitions(&configure,
        &RunContext { run_id: run3, nav_date: NaiveDate::from_ymd_opt(2026, 8, 21).unwrap(),
                      actor_label: "system", actor_user_id: None },
        &[Transition::Close { id }], &HashMap::new()).await.unwrap();

    let row = &scoped.breaches_for(&view, None).await.unwrap()[0];
    assert_eq!(row.closed_nav_date, NaiveDate::from_ymd_opt(2026, 8, 14),
        "the original close date must stand");
    let kinds: Vec<String> = scoped.breach_events(&view, id).await.unwrap()
        .into_iter().map(|e| e.event).collect();
    assert_eq!(kinds, vec!["opened", "cleared"],
        "an episode cleared twice in its own timeline is a falsified record: {kinds:?}");

    edb.stop().await;
}
