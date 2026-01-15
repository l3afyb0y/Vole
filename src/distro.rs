use std::fs;

#[derive(Debug, Clone, Default)]
pub struct Distro {
    pub id: Option<String>,
    pub id_like: Vec<String>,
}

impl Distro {
    pub fn identifiers(&self) -> Vec<String> {
        let mut ids = Vec::new();
        if let Some(id) = &self.id {
            ids.push(id.to_lowercase());
        }
        for item in &self.id_like {
            ids.push(item.to_lowercase());
        }
        ids
    }
}

pub fn detect() -> Distro {
    let content = fs::read_to_string("/etc/os-release").unwrap_or_default();
    let mut distro = Distro::default();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, '=');
        let key = parts.next().unwrap_or("");
        let value = parts.next().unwrap_or("");
        let value = value.trim().trim_matches('"');

        match key {
            "ID" => distro.id = Some(value.to_string()),
            "ID_LIKE" => {
                distro.id_like = value.split_whitespace().map(|s| s.to_string()).collect();
            }
            _ => {}
        }
    }

    distro
}
