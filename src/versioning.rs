use crate::model::{AppResult, Impact};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

pub fn next_version(current: &str, impact: Impact) -> AppResult<String> {
    let mut version = parse_version(current)?;
    let increment = |part: u64| {
        part.checked_add(1)
            .ok_or_else(|| format!("version '{current}' is too large to bump"))
    };

    match impact {
        Impact::Major => {
            version.major = increment(version.major)?;
            version.minor = 0;
            version.patch = 0;
        }
        Impact::Minor => {
            version.minor = increment(version.minor)?;
            version.patch = 0;
        }
        Impact::Patch => version.patch = increment(version.patch)?,
    }

    Ok(version.to_string())
}

pub fn parse_version(current: &str) -> AppResult<Version> {
    let mut parts = current.split('.');
    let major = parse_part(current, parts.next())?;
    let minor = parse_part(current, parts.next())?;
    let patch = parse_part(current, parts.next())?;
    if parts.next().is_some() {
        return Err(format!("invalid version '{current}'"));
    }

    Ok(Version {
        major,
        minor,
        patch,
    })
}

fn parse_part(current: &str, part: Option<&str>) -> AppResult<u64> {
    part.ok_or_else(|| format!("invalid version '{current}'"))?
        .parse::<u64>()
        .map_err(|_| format!("invalid version '{current}'"))
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}
