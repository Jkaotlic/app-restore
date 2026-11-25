//! Read native app icons from SpringBoard's pages, dock and folder pages.
use plist::Value;
use serde::Serialize;
use std::collections::HashSet;

#[derive(Serialize, Debug, PartialEq)]
pub struct HomeIcon {
    pub bundle_id: String,
    pub name: String,
}

pub fn app_icons(state: &Value) -> Option<Vec<HomeIcon>> {
    fn visit(v: &Value, icons: &mut Vec<HomeIcon>, seen: &mut HashSet<String>) {
        if let Some(page) = v.as_array() {
            for child in page {
                visit(child, icons, seen);
            }
            return;
        }
        let Some(icon) = v.as_dictionary() else { return };
        let field = |k: &str| icon.get(k).and_then(Value::as_string).map(str::trim).filter(|s| !s.is_empty());
        if field("listType") == Some("folder") {
            if let Some(pages) = icon.get("iconLists") {
                visit(pages, icons, seen);
            }
            return;
        }
        if icon.contains_key("webClipURL") || icon.contains_key("widgetIdentifier") {
            return;
        }
        let Some(id) = field("bundleIdentifier") else { return };
        if id.starts_with("com.apple.") || field("displayIdentifier").is_some_and(|other| other != id) || !seen.insert(id.into()) {
            return;
        }
        icons.push(HomeIcon { bundle_id: id.into(), name: field("displayName").unwrap_or(id).into() });
    }
    state.as_array()?;
    let mut icons = Vec::new();
    visit(state, &mut icons, &mut HashSet::new());
    icons.sort_by_cached_key(|a| a.name.to_lowercase());
    Some(icons)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_folders_and_dock_without_shortcuts_or_duplicates() {
        let state = serde_json::json!([[{"bundleIdentifier":"test.app","displayName":"Тест"}],
            [{"listType":"folder","iconLists":[[{"bundleIdentifier":"test.app"},
            {"bundleIdentifier":"other.app"},{"webClipURL":"https://example.com","bundleIdentifier":"test.web"},
            {"bundleIdentifier":"com.apple.Preferences"},{"displayIdentifier":"widget-id","bundleIdentifier":"test.widget"}]]}]]);
        let icons = app_icons(&serde_json::from_value(state).unwrap()).unwrap();
        assert_eq!(icons.len(), 2);
        assert_eq!(icons[0].name, "other.app");
        assert_eq!(icons[1].name, "Тест");
        assert!(app_icons(&Value::Boolean(false)).is_none());
    }
}
