//! The operator's commands.

use crate::common;

/// The operator's commands act on the catalog file, with the tracker stopped or running.
#[test]
fn operator_commands_hide_ban_and_list_reports() {
    use tempest_protocol::Hash;
    use tempest_tracker::{Catalog, CatalogConfig};
    use tempest_tracker_api::listing::{Content, NewReport};

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("catalog.db");
    let (publisher, reporter) = (Hash::new([1; 32]), Hash::new([2; 32]));
    let (first, second) = (Hash::new([0xa1; 32]), Hash::new([0xa2; 32]));
    {
        let mut catalog = Catalog::open(&file, CatalogConfig::default()).unwrap();
        for bundle in [&first, &second] {
            catalog.request_listing(100, bundle, &publisher).unwrap();
            let content = Content {
                name: "data".into(),
                size: 1,
                files: 1,
            };
            catalog.set_content(bundle, &content).unwrap();
        }
        catalog
            .request_listing(100, &Hash::new([9; 32]), &reporter)
            .unwrap();
        let report = NewReport {
            reason: "looks like malware\nsee the readme".into(),
        };
        catalog.report(200, &first, &reporter, &report).unwrap();
    }
    let operate = |args: &[&str]| {
        let output = common::tracker()
            .args(args)
            .arg("--catalog")
            .arg(&file)
            .output()
            .unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        (output.status.success(), stdout, stderr)
    };

    // Reports, with the tracker stopped.
    let (ok, out, _) = operate(&["reports"]);
    assert!(ok);
    assert!(out.contains(&format!("bundle     {first}")), "{out}");
    assert!(out.contains(&format!("reporter   {reporter}")), "{out}");
    assert!(
        out.contains("  looks like malware\n  see the readme"),
        "{out}"
    );
    let (_, out, _) = operate(&["reports", "--bundle", &second.to_string()]);
    assert_eq!(out.trim(), "no reports");

    // Hiding and banning, with the tracker running.
    let (_tracker, url) = common::start_on(&file);
    let status = |bundle: &Hash| common::get(&url, &format!("/tempest/v1/listings/{bundle}")).0;
    assert_eq!(status(&first), 200);
    let found = || {
        common::get(&url, "/tempest/v1/listings").1["total"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(found(), 2);

    let (ok, out, err) = operate(&["hide", &first.to_string()]);
    assert!(ok, "{err}");
    assert_eq!(out.trim(), format!("hid {first}"));
    assert_eq!(status(&first), 404);
    assert_eq!(status(&second), 200);
    assert_eq!(found(), 1);
    let (_, out, _) = operate(&["reports"]);
    assert!(out.contains("(taken down)"), "{out}");
    let (ok, _, _) = operate(&["unhide", &first.to_string()]);
    assert!(ok);
    assert_eq!(status(&first), 200);
    assert_eq!(found(), 2);

    let (ok, out, err) = operate(&["ban", &publisher.to_string()]);
    assert!(ok, "{err}");
    assert_eq!(out.trim(), format!("banned {publisher}"));
    assert_eq!(status(&first), 404);
    assert_eq!(status(&second), 404);
    assert_eq!(found(), 0);
    let (ok, out, _) = operate(&["unban", &publisher.to_string()]);
    assert!(ok);
    assert_eq!(out.trim(), format!("unbanned {publisher}"));
    assert_eq!(status(&second), 200);
    assert_eq!(found(), 2);

    // Mistakes are errors, and never make a catalog.
    let (ok, _, err) = operate(&["hide", &Hash::new([7; 32]).to_string()]);
    assert!(!ok);
    assert!(err.contains("no such listing"), "{err}");
    let missing = dir.path().join("none.db");
    let output = common::tracker()
        .args(["reports", "--catalog"])
        .arg(&missing)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!missing.exists());
}
