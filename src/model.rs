use std::collections::HashSet;
use std::path::PathBuf;
use std::str::FromStr;

pub type AppResult<T> = Result<T, String>;

#[derive(Debug, Clone)]
pub struct Config {
    pub paths: Vec<PathBuf>,
    pub ignored_directories: Vec<PathBuf>,
    pub major_types: HashSet<String>,
    pub minor_types: HashSet<String>,
    pub patch_types: HashSet<String>,
    pub skip_scopes: HashSet<String>,
    pub commit: bool,
    pub tag: bool,
    pub push: bool,
    pub force: bool,
    pub force_bump_type: Impact,
    pub allow_dirty: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Impact {
    Patch,
    Minor,
    Major,
}

impl Impact {
    pub fn as_str(self) -> &'static str {
        match self {
            Impact::Patch => "patch",
            Impact::Minor => "minor",
            Impact::Major => "major",
        }
    }
}

impl FromStr for Impact {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "patch" => Ok(Impact::Patch),
            "minor" => Ok(Impact::Minor),
            "major" => Ok(Impact::Major),
            _ => Err(format!(
                "invalid bump type '{value}': expected patch, minor, or major"
            )),
        }
    }
}
