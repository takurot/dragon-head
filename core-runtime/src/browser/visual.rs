use super::*;

impl PageSession {
    /// Explicitly request a visual capture with SoM marks.
    pub fn get_visual(&self) -> Result<VisualCapture> {
        self.capture_som(SomTrigger::GetVisual)
    }

    /// Returns the number of SoM captures generated for this page session.
    pub fn som_generation_count(&self) -> usize {
        self.som_pipeline
            .lock()
            .map(|state| state.generation_count)
            .unwrap_or_default()
    }

    /// Returns the latest SoM capture, if any.
    pub fn last_visual_capture(&self) -> Option<VisualCapture> {
        self.som_pipeline
            .lock()
            .ok()
            .and_then(|state| state.last_capture.clone())
    }

    pub(super) fn capture_som(&self, trigger: SomTrigger) -> Result<VisualCapture> {
        let root = self.get_document_node()?;
        let viewport = self.get_viewport_dimensions();
        let semantic_root = normalize_dom_with_viewport(LoadProfile::Visual, &root, viewport)?;

        let mut marks = Vec::new();
        self.collect_som_marks(&semantic_root, &mut marks);

        let image_png = self
            .inner
            .capture_screenshot(
                headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption::Png,
                None,
                None,
                true,
            )
            .context("Failed to capture SoM screenshot")?;

        let capture = VisualCapture {
            trigger,
            marks,
            image_png,
        };

        self.audit_logger.log(AuditEvent::VisualCapture {
            trigger: match capture.trigger {
                SomTrigger::GetVisual => "get_visual".to_string(),
                SomTrigger::ActAmbiguous => "act_ambiguous".to_string(),
                SomTrigger::VerifyFailed => "verify_failed".to_string(),
            },
            marks_count: capture.marks.len(),
            timestamp: epoch_millis_u64(),
        });

        self.store_som_capture(capture.clone());
        Ok(capture)
    }

    pub(super) fn trigger_som_capture_best_effort(&self, trigger: SomTrigger) {
        if let Err(err) = self.capture_som(trigger) {
            tracing::warn!(?trigger, error = %format!("{err:#}"), "SoM capture trigger failed");
        }
    }

    pub(super) fn store_som_capture(&self, capture: VisualCapture) {
        if let Ok(mut state) = self.som_pipeline.lock() {
            state.generation_count += 1;
            state.last_capture = Some(capture);
        }
    }

    pub(super) fn collect_som_marks(&self, node: &SemanticNode, out: &mut Vec<SomMark>) {
        if node.backend_node_id > 0 && node.stable_key.is_some() {
            if let Ok(Some(bbox)) = self.resolve_node_bbox(node.backend_node_id) {
                out.push(SomMark {
                    id: node.backend_node_id,
                    stable_key: node.stable_key.clone(),
                    bbox,
                });
            }
        }

        for child in &node.children {
            self.collect_som_marks(child, out);
        }
    }
}
