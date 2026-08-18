fn main() {
    let db: db::Db = unimplemented!();
    // No AuthCtx: there is no constructor for Scoped and no accessible pool.
    let _ = db.pool();
}
