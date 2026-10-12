//! Batch bounds, index resolution and the `Inexact` consequence (R9, R10, R12).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use super::*;

// --- R9: row and byte bounds ------------------------------------------------------------------

#[tokio::test]
async fn provider_splits_twenty_thousand_rows_into_bounded_batches() {
    let fx = fixture();
    put_all(
        &fx,
        (0..20_000_u64)
            .map(|i| (10 * HOUR + i, record(10 * HOUR + i, 1, "bash")))
            .collect(),
    );
    let p = provider(&fx);
    let batches = run(plan_for(&p, None, &[]).await).await;
    let sizes: Vec<usize> = batches.iter().map(RecordBatch::num_rows).collect();
    assert_eq!(sizes, vec![8192, 8192, 3616]);
}

#[tokio::test]
async fn provider_closes_a_batch_on_bytes_before_the_row_bound() {
    let fx = fixture();
    let fat = "x".repeat(300 * 1024);
    put_all(
        &fx,
        (0..11_u64)
            .map(|i| {
                let mut r = record(10 * HOUR + i, 1, "bash");
                r.command_line = Some(fat.clone());
                (10 * HOUR + i, r)
            })
            .collect(),
    );
    let cap = 1024 * 1024;
    let p = provider_with(
        &fx,
        ScanLimits {
            batch_max_bytes: cap,
            ..ScanLimits::default()
        },
    );
    let batches = run(plan_for(&p, None, &[]).await).await;
    let sizes: Vec<usize> = batches.iter().map(RecordBatch::num_rows).collect();
    // Three 300 KiB rows fit under 1 MiB, a fourth does not; the row bound (8192) is nowhere near.
    assert_eq!(sizes, vec![3, 3, 3, 2]);
    // The cap bounds the estimated data bytes; Arrow's builders over-allocate string buffers by up
    // to 2x (measured 1.34x here), so the allocated size is held to twice the cap.
    assert!(
        batches
            .iter()
            .all(|b| b.get_array_memory_size() <= cap.saturating_mul(2)),
        "every batch must stay within the cap plus Arrow's buffer slack"
    );
}

#[tokio::test]
async fn provider_excludes_one_oversized_row_and_still_scans_the_rest() {
    let fx = fixture();
    let rows_in: Vec<(u64, ProcessRecord)> = (0..6_u64)
        .map(|i| {
            let mut r = record(10 * HOUR + i, 1, "bash");
            if i == 2 {
                r.command_line = Some("y".repeat(2 * 1024 * 1024));
            }
            (10 * HOUR + i, r)
        })
        .collect();
    put_all(&fx, rows_in);
    let p = provider_with(
        &fx,
        ScanLimits {
            batch_max_bytes: 1024 * 1024,
            ..ScanLimits::default()
        },
    );
    let batches = run(plan_for(&p, None, &[]).await).await;
    assert_eq!(rows(&batches), 5);
    assert_eq!(p.counters().oversized_rows(), 1);
    assert_eq!(p.counters().rows_read(), 6);
    assert_eq!(p.counters().table(), "processes");
}

#[test]
fn provider_batch_max_bytes_default_admits_the_documented_worst_case_row() {
    let mut worst = ProcessRecord::new(1, "n".repeat(255));
    worst.executable_path = Some("p".repeat(4096).into());
    worst.command_line = Some("c".repeat(1024 * 1024));
    worst.executable_hash = Some(HASH_A.to_owned());
    let estimate = estimated_row_bytes(&worst);
    assert!(estimate > 1024 * 1024);
    assert!(estimate <= EXECUTOR_BATCH_MAX_BYTES);
    assert!(EXECUTOR_BATCH_MAX_BYTES >= estimate.saturating_mul(2));
}

#[test]
fn provider_row_estimate_never_undercounts_the_arrow_batch() {
    let schema = schema_for(&process_table()).unwrap();
    let typical: Vec<ProcessRecord> = (0..1000_u32)
        .map(|i| {
            let mut r = record(u64::from(i), i, "sshd");
            r.executable_path = Some("/usr/sbin/sshd".into());
            r.command_line = Some("sshd -D -f /etc/ssh/sshd_config".to_owned());
            r.executable_hash = Some(HASH_B.to_owned());
            r
        })
        .collect();
    let batch = self::arrow::encode(&typical, &schema, None).unwrap();
    let estimate: usize = typical.iter().map(estimated_row_bytes).sum();
    assert!(estimate >= batch.get_array_memory_size());
}

// --- R10 consequence, R12 ---------------------------------------------------------------------

#[tokio::test]
async fn provider_ae4_posting_admits_bash_and_filter_exec_rejects_it() {
    let fx = fixture();
    put_all(&fx, vec![(10 * HOUR, record(10 * HOUR, 7, "bash"))]);
    let p = provider(&fx);

    // The scan alone, with the rule's predicate: the case-insensitive index admits the row.
    let direct = run(plan_for(&p, None, &[col("name").eq(lit("Bash"))]).await).await;
    assert_eq!(rows(&direct), 1);

    // Through the planner the FilterExec re-checks it and rejects it.
    let ctx = SessionContext::new();
    ctx.register_table("processes", Arc::<EventStoreTableProvider>::clone(&p))
        .unwrap();
    let df = ctx
        .sql("SELECT pid FROM processes WHERE name = 'Bash'")
        .await
        .unwrap();
    let physical = df.clone().create_physical_plan().await.unwrap();
    let shown = displayable(physical.as_ref()).indent(false).to_string();
    assert!(shown.contains("FilterExec"));
    assert!(shown.contains("BucketScanExec"));
    let before = p.counters().rows_read();
    let collected = df.collect().await.unwrap();
    assert_eq!(rows(&collected), 0);
    assert_eq!(p.counters().rows_read().saturating_sub(before), 1);

    let exact = ctx
        .sql("SELECT pid FROM processes WHERE name = 'bash'")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(rows(&exact), 1);
}

#[tokio::test]
async fn provider_intersects_posting_lists_before_decoding_a_row() {
    let fx = fixture();
    // pid 7 on i in 0..10 (10 rows), name bash on i in 0..3 and 10..17 (10 rows); both on 0..3.
    let rows_in: Vec<(u64, ProcessRecord)> = (0..30_u64)
        .map(|i| {
            let pid = if i < 10 {
                7
            } else {
                100 + u32::try_from(i).unwrap()
            };
            let name = if i < 3 || (10..17).contains(&i) {
                "bash"
            } else {
                "zsh"
            };
            (10 * HOUR + i, record(10 * HOUR + i, pid, name))
        })
        .collect();
    put_all(&fx, rows_in);
    let reader = fx.store.open_read().unwrap();
    assert_eq!(reader.postings(10, IndexTerm::Pid(7)).unwrap().len(), 10);
    assert_eq!(
        reader.postings(10, IndexTerm::name("bash")).unwrap().len(),
        10
    );

    let p = provider(&fx);
    let ctx = SessionContext::new();
    ctx.register_table("processes", Arc::<EventStoreTableProvider>::clone(&p))
        .unwrap();
    let out = ctx
        .sql("SELECT pid FROM processes WHERE pid = 7 AND name = 'bash'")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(rows(&out), 3);
    // Fewer rows decoded than either posting list alone: the intersection, not the filter, did it.
    assert_eq!(p.counters().rows_read(), 3);
}

#[tokio::test]
async fn provider_resolves_ppid_hash_and_in_list_through_their_indexes() {
    let fx = fixture();
    let rows_in: Vec<(u64, ProcessRecord)> = (0..10_u64)
        .map(|i| {
            let name = match i {
                0 => "alpha",
                1 => "beta",
                _ => "other",
            };
            let mut r = record(10 * HOUR + i, u32::try_from(i).unwrap(), name);
            r.ppid = Some(crate::models::ProcessId::new(if i < 2 { 50 } else { 51 }));
            r.executable_hash = Some(if i == 4 { HASH_A } else { HASH_B }.to_owned());
            (10 * HOUR + i, r)
        })
        .collect();
    put_all(&fx, rows_in);

    let by_name = provider(&fx);
    let name_filter = col("name").in_list(vec![lit("ALPHA"), lit("beta")], false);
    assert_eq!(
        rows(&run(plan_for(&by_name, None, &[name_filter]).await).await),
        2
    );
    assert_eq!(by_name.counters().rows_read(), 2);

    let by_ppid = provider(&fx);
    let ppid_filter = col("ppid").eq(lit(50_u64));
    assert_eq!(
        rows(&run(plan_for(&by_ppid, None, &[ppid_filter]).await).await),
        2
    );
    assert_eq!(by_ppid.counters().rows_read(), 2);

    let by_hash = provider(&fx);
    let hash_filter = col("executable_hash").eq(lit(HASH_A));
    assert_eq!(
        rows(&run(plan_for(&by_hash, None, &[hash_filter]).await).await),
        1
    );
    assert_eq!(by_hash.counters().rows_read(), 1);
}

#[tokio::test]
async fn provider_caches_closed_bucket_postings_and_reads_the_open_bucket_live() {
    let fx = fixture();
    let now = u64::try_from(Utc::now().timestamp_millis()).unwrap();
    put_all(
        &fx,
        vec![
            (10 * HOUR, record(10 * HOUR, 5, "bash")),
            (now, record(now, 5, "bash")),
        ],
    );
    let p = provider(&fx);
    for _ in 0..2 {
        let plan = plan_for(&p, None, &[col("pid").eq(lit(5_u64))]).await;
        assert_eq!(rows(&run(plan).await), 2);
    }
    // The closed bucket's list was loaded once and served once; the open bucket's never stored.
    assert_eq!(fx.cache.len(), 1);
    assert_eq!(fx.cache.misses(), 1);
    assert_eq!(fx.cache.hits(), 1);
    assert_eq!(fx.cache.bypassed_open(), 2);
}

#[tokio::test]
async fn provider_reloads_a_closed_bucket_list_after_a_late_write_into_it() {
    let fx = fixture();
    put_all(&fx, vec![(10 * HOUR, record(10 * HOUR, 5, "bash"))]);
    let p = provider(&fx);
    let before = plan_for(&p, None, &[col("pid").eq(lit(5_u64))]).await;
    assert_eq!(rows(&run(before).await), 1);

    // A clock step-back lands a row in the closed bucket after its list was cached.
    put_all(&fx, vec![(10 * HOUR + 1, record(10 * HOUR + 1, 5, "bash"))]);
    let after = plan_for(&p, None, &[col("pid").eq(lit(5_u64))]).await;
    assert_eq!(
        rows(&run(after).await),
        2,
        "the cached list must not hide the late row"
    );
    assert_eq!(fx.cache.misses(), 2);
}

#[tokio::test]
async fn provider_clamps_rows_to_the_time_window_inside_a_bucket() {
    let fx = fixture();
    put_all(
        &fx,
        (0..6_u64)
            .map(|i| (10 * HOUR + i, record(10 * HOUR + i, 1, "bash")))
            .collect(),
    );
    let p = provider(&fx);
    let filters = [
        col("collection_time").gt_eq(lit(ms(10 * HOUR + 2))),
        col("collection_time").lt(lit(ms(10 * HOUR + 4))),
    ];
    assert_eq!(rows(&run(plan_for(&p, None, &filters).await).await), 2);
}

#[tokio::test]
async fn provider_honours_projection_including_the_empty_one() {
    let fx = fixture();
    put_all(
        &fx,
        (0..4_u64)
            .map(|i| (10 * HOUR + i, record(10 * HOUR + i, 1, "bash")))
            .collect(),
    );
    let p = provider(&fx);
    let one = run(plan_for(&p, Some(&vec![2]), &[]).await).await;
    assert_eq!(one[0].num_columns(), 1);
    assert_eq!(one[0].schema().field(0).name(), "name");
    let none = run(plan_for(&p, Some(&vec![]), &[]).await).await;
    assert_eq!(none[0].num_columns(), 0);
    assert_eq!(rows(&none), 4);
}
