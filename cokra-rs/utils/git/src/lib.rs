// Utils - Git
// Git utilities

use git2::Repository;
use anyhow::Result;

/// Get current git branch
pub fn current_branch() -> Result<String> {
    let repo = Repository::open(".")?;
    let head = repo.head()?;
    let shorthand = head.shorthand()?;
    Ok(shorthand.to_string())
}

/// Get current git commit hash
pub fn current_commit() -> Result<String> {
    let repo = Repository::open(".")?;
    let obj = repo.head()?.peel_to_commit()?;
    Ok(obj.id().to_string())
}
