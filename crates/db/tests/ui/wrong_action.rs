use db::auth::marker::{Shareholders, View};
use db::auth::Access;
use db::scoped::Scoped;

async fn f(s: Scoped<'_>, view_token: Access<Shareholders, View>) {
    // Writing requires an Import token, not a View token.
    let _ = s.shareholders_replace(&view_token, &[]).await;
}

fn main() {}
