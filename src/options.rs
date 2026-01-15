use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DownloadsChoice {
    Archives,
    Folders,
}

impl DownloadsChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            DownloadsChoice::Archives => "archives",
            DownloadsChoice::Folders => "folders",
        }
    }
}

impl std::fmt::Display for DownloadsChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ScanOptions {
    pub downloads_choice: Option<DownloadsChoice>,
}
