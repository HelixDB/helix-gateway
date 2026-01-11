pub mod db_integration_test;
pub mod integration_test;

#[tokio::test]
async fn test_watch() {
    let (tx, mut rx) = tokio::sync::watch::channel(false);

    // First send - value changes from false to true
    tx.send_if_modified(|current| {
        if *current != true {
            *current = true;
            true // notify
        } else {
            false // don't notify
        }
    });

    let _ = rx.changed().await; // rx sees change

    // Second send - same value, no notification
    tx.send_if_modified(|current| {
        if *current != true {
            *current = true;
            true
        } else {
            false // value unchanged, don't notify
        }
    });

    // other_rx should NOT see a new change from the second send
    assert!(!rx.has_changed().unwrap());
}
