use anyhow::{Result, bail, ensure};

fn shell_escape(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SshRemotePathKind<'a> {
    Absolute(&'a str),
    Home,
    HomeRelative(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SshRemotePath<'a> {
    raw: &'a str,
    kind: SshRemotePathKind<'a>,
}

impl<'a> SshRemotePath<'a> {
    pub(crate) fn parse(path: &'a str, field: &str) -> Result<Self> {
        let path = path.trim();
        ensure!(!path.is_empty(), "{field} cannot be empty");
        ensure!(!path.contains('\0'), "{field} cannot contain NUL bytes");

        let kind = if path.starts_with('/') {
            SshRemotePathKind::Absolute(path)
        } else if path == "~" {
            SshRemotePathKind::Home
        } else if let Some(rest) = path.strip_prefix("~/") {
            SshRemotePathKind::HomeRelative(rest)
        } else if path.starts_with('~') {
            bail!("{field} only supports ~ or ~/..., not user-relative paths: {field}={path}");
        } else {
            bail!("{field} must be an absolute path, ~, or ~/...: {field}={path}");
        };

        Ok(Self { raw: path, kind })
    }

    pub(crate) fn input(self) -> &'a str {
        self.raw
    }

    pub(crate) fn kind(self) -> SshRemotePathKind<'a> {
        self.kind
    }

    pub(crate) fn render_for_shell(self) -> String {
        match self.kind {
            SshRemotePathKind::Absolute(path) => shell_escape(path),
            SshRemotePathKind::Home => "\"${HOME:-~}\"".to_string(),
            SshRemotePathKind::HomeRelative("") => "\"${HOME:-~}\"".to_string(),
            SshRemotePathKind::HomeRelative(rest) => {
                format!("\"${{HOME:-~}}\"/{}", shell_escape(rest))
            }
        }
    }

    pub(crate) fn resolve_with_home(self, home: &str) -> String {
        match self.kind {
            SshRemotePathKind::Absolute(path) => path.to_string(),
            SshRemotePathKind::Home => home.to_string(),
            SshRemotePathKind::HomeRelative("") => home.to_string(),
            SshRemotePathKind::HomeRelative(rest) if home == "/" => format!("/{rest}"),
            SshRemotePathKind::HomeRelative(rest) => format!("{home}/{rest}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SshRemotePath, SshRemotePathKind};

    #[test]
    fn parses_supported_remote_path_forms() {
        assert_eq!(
            SshRemotePath::parse("/srv/app", "field").unwrap().kind(),
            SshRemotePathKind::Absolute("/srv/app")
        );
        assert_eq!(
            SshRemotePath::parse("~", "field").unwrap().kind(),
            SshRemotePathKind::Home
        );
        assert_eq!(
            SshRemotePath::parse("~/repo", "field").unwrap().kind(),
            SshRemotePathKind::HomeRelative("repo")
        );
        assert_eq!(
            SshRemotePath::parse("~/", "field").unwrap().kind(),
            SshRemotePathKind::HomeRelative("")
        );
    }

    #[test]
    fn rejects_unsupported_remote_path_forms() {
        let relative = SshRemotePath::parse("repo", "field").expect_err("relative path");
        assert!(format!("{relative:#}").contains("absolute path, ~, or ~/..."));

        let dot_relative = SshRemotePath::parse("./repo", "field").expect_err("dot-relative");
        assert!(format!("{dot_relative:#}").contains("absolute path, ~, or ~/..."));

        let user_relative =
            SshRemotePath::parse("~alice/repo", "field").expect_err("user-relative");
        assert!(format!("{user_relative:#}").contains("only supports ~ or ~/..."));

        let empty = SshRemotePath::parse(" ", "field").expect_err("empty path");
        assert!(format!("{empty:#}").contains("cannot be empty"));
    }

    #[test]
    fn renders_home_relative_paths_for_remote_shells() {
        assert_eq!(
            SshRemotePath::parse("~", "field")
                .unwrap()
                .render_for_shell(),
            "\"${HOME:-~}\""
        );
        assert_eq!(
            SshRemotePath::parse("~/repo dir", "field")
                .unwrap()
                .render_for_shell(),
            "\"${HOME:-~}\"/'repo dir'"
        );
    }

    #[test]
    fn resolves_home_relative_paths_against_remote_home() {
        assert_eq!(
            SshRemotePath::parse("~/", "field")
                .unwrap()
                .resolve_with_home("/home/alice"),
            "/home/alice"
        );
        assert_eq!(
            SshRemotePath::parse("~/repo", "field")
                .unwrap()
                .resolve_with_home("/home/alice"),
            "/home/alice/repo"
        );
        assert_eq!(
            SshRemotePath::parse("~/repo", "field")
                .unwrap()
                .resolve_with_home("/"),
            "/repo"
        );
    }
}
