use sic_core_rust::storage::Store;
#[tokio::test]
async fn linked_sqlite_runtime_meets_the_patched_wal_release_baseline(){
 let store=Store::memory().await.unwrap();let mut tx=store.begin().await.unwrap();let version:String=sqlx::query_scalar("SELECT sqlite_version()").fetch_one(&mut *tx).await.unwrap();
 let parts:Vec<u32>=version.split('.').map(|part|part.parse().unwrap()).collect();assert!(parts.as_slice()>=[3,54,0].as_slice(),"linked SQLite {version} predates the patched 3.54.0 baseline");
 let source_id:String=sqlx::query_scalar("SELECT sqlite_source_id()").fetch_one(&mut *tx).await.unwrap();assert!(!source_id.is_empty());
 println!("linked_sqlite_version={version}; linked_sqlite_source_id={source_id}");
}
