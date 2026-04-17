use std::collections::HashSet;
use std::path::PathBuf;

pub type AppResult<T> = Result<T, String>;

#[derive(Debug, Clone)]
pub struct Config {
    pub paths: Vec<PathBuf>,
    pub major_types: HashSet<String>,
    pub minor_types: HashSet<String>,
    pub patch_types: HashSet<String>,
    pub skip_scopes: HashSet<String>,
    pub commit: bool,
    pub tag: bool,
    pub push: bool,
    pub force: bool,
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
