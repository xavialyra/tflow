use ratatui::layout::Rect;

/// Host-owned rectangles shared by rendering and runtime Resize delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PaneLayout {
    pub(crate) omnibar: Option<Rect>,
    pub(crate) divider: Option<Rect>,
    pub(crate) body: Rect,
    pub(crate) primary: Rect,
    pub(crate) companion: Option<Rect>,
    pub(crate) separator: Option<Rect>,
}

impl PaneLayout {
    pub(crate) fn new(
        area: Rect,
        input_visible: bool,
        show_divider: bool,
        companion: bool,
    ) -> Self {
        let input_rows = u16::from(input_visible).min(area.height);
        let divider_rows =
            u16::from(input_visible && show_divider).min(area.height.saturating_sub(input_rows));
        let omnibar = (input_rows > 0).then(|| Rect::new(area.x, area.y, area.width, input_rows));
        let divider = (divider_rows > 0).then(|| {
            Rect::new(
                area.x,
                area.y.saturating_add(input_rows),
                area.width,
                divider_rows,
            )
        });
        let body = Rect::new(
            area.x,
            area.y.saturating_add(input_rows + divider_rows),
            area.width,
            area.height.saturating_sub(input_rows + divider_rows),
        );
        let (primary, companion, separator) = if companion && body.width >= 3 && body.height > 0 {
            let left = body.width / 2;
            (
                Rect::new(body.x, body.y, left, body.height),
                Some(Rect::new(
                    body.x.saturating_add(left + 1),
                    body.y,
                    body.width - left - 1,
                    body.height,
                )),
                Some(Rect::new(
                    body.x.saturating_add(left),
                    body.y,
                    1,
                    body.height,
                )),
            )
        } else {
            (body, None, None)
        };
        Self {
            omnibar,
            divider,
            body,
            primary,
            companion,
            separator,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_is_full_width_and_panes_share_one_body() {
        let layout = PaneLayout::new(Rect::new(1, 1, 78, 22), true, true, true);
        assert_eq!(layout.omnibar, Some(Rect::new(1, 1, 78, 1)));
        assert_eq!(layout.divider, Some(Rect::new(1, 2, 78, 1)));
        assert_eq!(layout.primary, Rect::new(1, 3, 39, 20));
        assert_eq!(layout.separator, Some(Rect::new(40, 3, 1, 20)));
        assert_eq!(layout.companion, Some(Rect::new(41, 3, 38, 20)));
    }

    #[test]
    fn tiny_surfaces_do_not_create_an_out_of_bounds_or_zero_width_pane() {
        for width in 0..=5 {
            for height in 0..=3 {
                let layout = PaneLayout::new(Rect::new(0, 0, width, height), true, true, true);
                assert!(layout.primary.right() <= width);
                assert!(layout.primary.bottom() <= height);
                if let Some(companion) = layout.companion {
                    assert!(companion.width > 0 && companion.height > 0);
                    assert!(companion.right() <= width && companion.bottom() <= height);
                }
            }
        }
    }
}
