//! A change of address reaches the profile once, and a stale or replayed event does nothing.

use event_contracts::payloads::EmailChangedV1;
use test_utilities::TestDb;
use user_service::{profile_sync::apply_email_change, store, MIGRATOR};
use uuid::Uuid;

#[tokio::test]
async fn email_changes_apply_only_over_the_address_they_replace() {
    let db = TestDb::create(&MIGRATOR).await.expect("test database");
    let user = Uuid::now_v7();
    store::ensure_profile(&db.app, user, "ada@example.com", "Ada").await.unwrap();
    let first = EmailChangedV1 { user_id: user, email: "ada@new.example".into(), previous_email: "ada@example.com".into() };
    let second = EmailChangedV1 { user_id: user, email: "ada@third.example".into(), previous_email: "ada@new.example".into() };

    assert!(apply_email_change(&db.app, &first).await.unwrap());
    assert!(!apply_email_change(&db.app, &first).await.unwrap(), "a replay changes nothing");
    assert!(apply_email_change(&db.app, &second).await.unwrap());
    assert!(!apply_email_change(&db.app, &first).await.unwrap(), "an older change does not undo a newer one");
    let profile = store::ensure_profile(&db.app, user, "stale@token.example", "Ada").await.unwrap();
    assert_eq!(profile.email, "ada@third.example");
    let unknown =
        EmailChangedV1 { user_id: Uuid::now_v7(), email: "x@example.com".into(), previous_email: "y@example.com".into() };
    assert!(!apply_email_change(&db.app, &unknown).await.unwrap());
    db.cleanup().await;
}
