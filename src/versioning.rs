use crate::model::{AppResult, Impact};

pub fn next_version(current: &str, impact: Impact) -> AppResult<String> {
    let mut parts = current.split('.');
    let mut major = parts
        .next()
        .ok_or_else(|| format!("invalid version '{current}'"))?
        .parse::<u64>()
        .map_err(|_| format!("invalid version '{current}'"))?;
    let mut minor = parts
        .next()
        .ok_or_else(|| format!("invalid version '{current}'"))?
        .parse::<u64>()
        .map_err(|_| format!("invalid version '{current}'"))?;
    let mut patch = parts
        .next()
        .ok_or_else(|| format!("invalid version '{current}'"))?
        .parse::<u64>()
        .map_err(|_| format!("invalid version '{current}'"))?;

    match impact {
        Impact::Major => {
            major += 1;
            minor = 0;
            patch = 0;
        }
        Impact::Minor => {
            minor += 1;
            patch = 0;
        }
        Impact::Patch => patch += 1,
    }

    Ok(format!("{major}.{minor}.{patch}"))
}
