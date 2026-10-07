use nostr::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoState {
    pub refs: Vec<(String, String)>,
    pub head: Option<String>,
}

impl RepoState {
    pub fn build(id: &str, refs: &[(String, String)], head: Option<&str>) -> EventBuilder {
        let mut tags: Vec<Tag> = vec![Tag::identifier(id.to_owned())];

        for (name, commit) in refs {
            tags.push(Tag::parse([name.as_str(), commit.as_str()]).expect("valid ref tag"));
        }

        if let Some(head) = head {
            tags.push(
                Tag::parse(["HEAD", &format!("ref: refs/heads/{head}")]).expect("valid HEAD tag"),
            );
        }

        EventBuilder::new(Kind::RepoState, "").tags(tags)
    }

    pub fn parse(event: &Event) -> Self {
        let mut refs = Vec::new();
        let mut head = None;

        for tag in event.tags.iter() {
            match Nip34Tag::parse(tag.as_slice()) {
                Ok(Nip34Tag::Head(branch)) => head = Some(branch),
                Ok(Nip34Tag::RefHead { branch, commit }) => {
                    refs.push((format!("refs/heads/{branch}"), commit.to_string()));
                }
                Ok(Nip34Tag::RefTag { name, commit }) => {
                    refs.push((format!("refs/tags/{name}"), commit.to_string()));
                }
                _ => {}
            }
        }

        Self { refs, head }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMIT_A: &str = "aa231c4c6a5777dc89b42207b499891a344add5c";
    const COMMIT_B: &str = "59429cfc6cb35b0a1ddace73b5a5c5ed57b8f5ca";

    fn keys() -> Keys {
        Keys::new(
            SecretKey::from_hex("0000000000000000000000000000000000000000000000000000000000000001")
                .expect("valid secret key"),
        )
    }

    fn state_event(tags: &[&[&str]]) -> Event {
        let tags: Vec<Tag> = tags
            .iter()
            .map(|t| Tag::parse(t.to_vec()).expect("valid tag"))
            .collect();

        EventBuilder::new(Kind::RepoState, "")
            .tags(tags)
            .finalize(&keys())
            .expect("signed event")
    }

    #[test]
    fn parses_heads_and_tags() {
        let event = state_event(&[
            &["HEAD", "ref: refs/heads/main"],
            &["refs/heads/main", COMMIT_A],
            &["refs/heads/dev", COMMIT_B],
            &["refs/tags/v1.0", COMMIT_A],
        ]);

        let state = RepoState::parse(&event);

        assert_eq!(state.head.as_deref(), Some("main"));
        assert_eq!(
            state.refs,
            vec![
                ("refs/heads/main".to_owned(), COMMIT_A.to_owned()),
                ("refs/heads/dev".to_owned(), COMMIT_B.to_owned()),
                ("refs/tags/v1.0".to_owned(), COMMIT_A.to_owned()),
            ]
        );
    }

    #[test]
    fn build_round_trips_through_parse() {
        let refs = [
            ("refs/heads/main".to_owned(), COMMIT_A.to_owned()),
            ("refs/heads/dev".to_owned(), COMMIT_B.to_owned()),
            ("refs/tags/v1.0".to_owned(), COMMIT_A.to_owned()),
        ];

        let event = RepoState::build("my-repo", &refs, Some("main"))
            .finalize(&keys())
            .expect("signed event");

        assert_eq!(event.kind, Kind::RepoState);
        assert_eq!(event.tags.identifier().as_deref(), Some("my-repo"));

        let state = RepoState::parse(&event);
        assert_eq!(state.refs, refs);
        assert_eq!(state.head.as_deref(), Some("main"));
    }
}
