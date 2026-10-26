use super::*;
use sha2::{Digest, Sha256};

pub(super) fn approval_url_fingerprint(url: &str) -> String {
    hex::encode(Sha256::digest(url.as_bytes()))
}

/// Private identity of the page reviewed for an element action. Store fingerprints rather
/// than copying query strings or data-document content into approval records or diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PageApprovalContext {
    revision: u64,
    frame_id: String,
    loader_id: String,
    url_fingerprint: String,
    full_url_fingerprint: String,
}

impl PageApprovalContext {
    pub(super) fn from_snapshot(snapshot: &DocumentSnapshot) -> Result<Self> {
        if snapshot.frame_id.is_empty() || snapshot.loader_id.is_empty() {
            anyhow::bail!("Policy approval document identity is unavailable");
        }
        let mut url = url::Url::parse(&snapshot.url)
            .map_err(|_| anyhow::anyhow!("Could not resolve the policy approval document"))?;
        let full_url_fingerprint = approval_url_fingerprint(url.as_str());
        url.set_fragment(None);
        Ok(Self {
            revision: snapshot.revision,
            frame_id: snapshot.frame_id.clone(),
            loader_id: snapshot.loader_id.clone(),
            url_fingerprint: approval_url_fingerprint(url.as_str()),
            full_url_fingerprint,
        })
    }

    pub(super) fn same_document(&self, other: &Self) -> bool {
        self.revision == other.revision
            && self.frame_id == other.frame_id
            && self.loader_id == other.loader_id
            && self.url_fingerprint == other.url_fingerprint
    }

    pub(super) fn matches(&self, other: &Self, scope: ApprovalScope) -> bool {
        self.same_document(other)
            && (scope != ApprovalScope::UntilNavigation
                || self.full_url_fingerprint == other.full_url_fingerprint)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ApprovalBinding {
    PageAction(PageApprovalContext),
    /// A navigation capability is already bound to the original/destination digest. It must
    /// survive intermediate documents while that exact approved navigation is retried.
    NavigationCapability,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(url: &str) -> DocumentSnapshot {
        DocumentSnapshot {
            frame_id: "frame".to_string(),
            loader_id: "loader".to_string(),
            url: url.to_string(),
            revision: 1,
        }
    }

    #[test]
    fn document_identity_and_scope_matrix() -> Result<()> {
        let original = document("https://example.com/page?query=one");
        let reviewed = PageApprovalContext::from_snapshot(&original)?;
        let scopes = [
            ApprovalScope::ActionOnly,
            ApprovalScope::UntilNavigation,
            ApprovalScope::Timeboxed { ms: 1_000 },
        ];
        let mut changed = vec![
            document("https://other.example/page?query=one"),
            document("https://example.com/other?query=one"),
            document("https://example.com/page?query=two"),
        ];
        let mut reload = original.clone();
        reload.loader_id = "new-loader".to_string();
        changed.push(reload);
        let mut aba = original.clone();
        aba.revision += 2;
        changed.push(aba);
        let mut frame = original.clone();
        frame.frame_id = "replacement-frame".to_string();
        changed.push(frame);
        for scope in scopes {
            assert!(reviewed.matches(&reviewed, scope));
            for change in &changed {
                assert!(!reviewed.matches(&PageApprovalContext::from_snapshot(change)?, scope));
            }
        }
        let fragment = PageApprovalContext::from_snapshot(&document(
            "https://example.com/page?query=one#fragment",
        ))?;
        assert!(reviewed.matches(&fragment, ApprovalScope::Timeboxed { ms: 1_000 }));
        assert!(!reviewed.matches(&fragment, ApprovalScope::UntilNavigation));
        Ok(())
    }

    #[test]
    fn internal_about_and_data_documents_are_supported_without_disclosing_content() -> Result<()> {
        for raw in ["about:blank", "data:text/html,private-content"] {
            let context = PageApprovalContext::from_snapshot(&document(raw))?;
            assert!(!format!("{context:?}").contains(raw));
        }
        assert!(PageApprovalContext::from_snapshot(&document("")).is_err());
        for field in ["frame", "loader"] {
            let mut missing = document("https://example.com/private?token=secret");
            if field == "frame" {
                missing.frame_id.clear();
            } else {
                missing.loader_id.clear();
            }
            let error = PageApprovalContext::from_snapshot(&missing).unwrap_err();
            assert_eq!(
                error.to_string(),
                "Policy approval document identity is unavailable"
            );
            assert!(!error.to_string().contains("secret"));
        }
        Ok(())
    }
}
