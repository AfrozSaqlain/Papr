//! Integration checks for the stable papr-core consumer surface.

use papr_core::{Database, PluginHost, PluginRequest};

#[test]
fn fresh_database_exposes_dashboard_through_public_api() -> Result<(), Box<dyn std::error::Error>> {
    let database = Database::in_memory()?;
    let dashboard = database.research_dashboard()?;
    assert_eq!(dashboard.counts.papers, 0);
    assert_eq!(dashboard.reading.heatmap.len(), 84);
    Ok(())
}

#[test]
fn empty_plugin_directory_is_a_valid_registry() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join(format!("papr-public-api-{}", std::process::id()));
    let host = PluginHost::discover(&root, &[])?;
    assert_eq!(host.plugins().len(), 1);
    assert_eq!(host.plugins()[0].id, "auto-tagger");
    assert!(host.diagnostics().is_empty());
    let request = PluginRequest::new("test", serde_json::json!({"ok": true}));
    assert_eq!(request.event, "test");
    std::fs::remove_dir_all(root)?;
    Ok(())
}
