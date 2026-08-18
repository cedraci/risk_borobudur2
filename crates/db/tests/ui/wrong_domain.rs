use db::auth::marker::{Nav, Positions, View};
use db::auth::Access;
use db::scoped::Scoped;

async fn f(s: Scoped<'_>, nav_token: Access<Nav, View>) {
    // A NAV authorization must not open a positions query.
    let _ = s.positions_for(&nav_token, chrono::NaiveDate::MIN).await;
    let _: Access<Positions, View>;
}

fn main() {}
