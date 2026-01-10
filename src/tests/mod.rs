pub mod db_integration_test;
pub mod integration_test;

#[tokio::test]
async fn test_watch() {
    let (tx, mut rx) = tokio::sync::watch::channel(());
    let other_rx = rx.clone();

    let _ = tx.send(());
    let _ = rx.changed().await;
    if other_rx.borrow().has_changed() {
        let _ = tx.send(());
        assert!(true)
    } else {
        assert!(false)
    }
}
