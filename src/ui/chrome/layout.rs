#[cfg(test)]
use super::clip;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Insets {
    pub(crate) top: usize,
    pub(crate) right: usize,
    pub(crate) bottom: usize,
    pub(crate) left: usize,
}

impl Insets {
    pub(crate) const ZERO: Self = Self {
        top: 0,
        right: 0,
        bottom: 0,
        left: 0,
    };

    pub(crate) const fn new(top: usize, right: usize, bottom: usize, left: usize) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }

    #[cfg(test)]
    pub(crate) fn horizontal(self) -> usize {
        self.left.saturating_add(self.right)
    }

    #[cfg(test)]
    pub(crate) fn vertical(self) -> usize {
        self.top.saturating_add(self.bottom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InputLayout {
    pub(crate) rows: usize,
    pub(crate) divider_rows: usize,
    pub(crate) padding: Insets,
    pub(crate) divider_padding: Insets,
}

impl Default for InputLayout {
    fn default() -> Self {
        Self {
            rows: 1,
            divider_rows: 1,
            padding: Insets::ZERO,
            divider_padding: Insets::ZERO,
        }
    }
}

#[cfg(test)]
impl InputLayout {
    fn input_region_rows(self) -> usize {
        self.rows.saturating_add(self.padding.vertical())
    }

    fn divider_region_rows(self) -> usize {
        self.divider_rows
            .saturating_add(self.divider_padding.vertical())
    }

    fn total_rows(self) -> usize {
        self.input_region_rows()
            .saturating_add(self.divider_region_rows())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChromeLayout {
    pub(crate) topbar_rows: usize,
    pub(crate) topbar_padding: Insets,
    pub(crate) input: InputLayout,
    pub(crate) footer_divider_rows: usize,
    pub(crate) footer_divider_padding: Insets,
    pub(crate) footer_rows: usize,
    pub(crate) viewport_padding: Insets,
    pub(crate) content_padding: Insets,
    pub(crate) footer_padding: Insets,
}

impl Default for ChromeLayout {
    fn default() -> Self {
        Self {
            topbar_rows: 1,
            topbar_padding: Insets::ZERO,
            input: InputLayout::default(),
            footer_divider_rows: 0,
            footer_divider_padding: Insets::ZERO,
            footer_rows: 1,
            viewport_padding: Insets::new(0, 1, 0, 1),
            content_padding: Insets::ZERO,
            footer_padding: Insets::ZERO,
        }
    }
}

#[cfg(test)]
impl ChromeLayout {
    fn region_rows(rows: usize, padding: Insets) -> usize {
        rows.saturating_add(padding.vertical())
    }

    pub(crate) fn topbar_row(self) -> usize {
        self.viewport_padding.top
    }

    pub(crate) fn topbar_content_row(self) -> usize {
        self.topbar_row().saturating_add(self.topbar_padding.top)
    }

    pub(crate) fn input_row(self) -> usize {
        self.topbar_row()
            .saturating_add(Self::region_rows(self.topbar_rows, self.topbar_padding))
    }

    pub(crate) fn input_content_row(self) -> usize {
        self.input_row().saturating_add(self.input.padding.top)
    }

    pub(crate) fn divider_row(self) -> usize {
        self.input_row()
            .saturating_add(self.input.input_region_rows())
    }

    pub(crate) fn divider_content_row(self) -> usize {
        self.divider_row()
            .saturating_add(self.input.divider_padding.top)
    }

    pub(crate) fn content_start_row(self) -> usize {
        self.input_row()
            .saturating_add(self.input.total_rows())
            .saturating_add(self.content_padding.top)
    }

    fn footer_region_row(self, height: usize) -> usize {
        height.saturating_sub(
            self.viewport_padding
                .bottom
                .saturating_add(Self::region_rows(self.footer_rows, self.footer_padding)),
        )
    }

    pub(crate) fn footer_divider_row(self, height: usize) -> usize {
        self.footer_region_row(height)
            .saturating_sub(Self::region_rows(
                self.footer_divider_rows,
                self.footer_divider_padding,
            ))
    }

    pub(crate) fn footer_row(self, height: usize) -> usize {
        self.footer_region_row(height)
            .saturating_add(self.footer_padding.top)
    }

    pub(crate) fn content_rows(self, height: usize) -> usize {
        let content_end = self.footer_divider_row(height);
        content_end.saturating_sub(
            self.content_start_row()
                .saturating_add(self.content_padding.bottom),
        )
    }

    pub(crate) fn viewport_width(self, width: usize) -> usize {
        width.saturating_sub(self.viewport_padding.horizontal())
    }

    pub(crate) fn content_width(self, width: usize) -> usize {
        self.viewport_width(width)
            .saturating_sub(self.content_padding.horizontal())
    }

    pub(crate) fn chrome_width(self, width: usize) -> usize {
        self.viewport_width(width)
    }

    pub(crate) fn pad_line(self, text: &str, width: usize, padding: Insets) -> String {
        let left = padding.left.min(width);
        let right = padding.right.min(width.saturating_sub(left));
        let available = width.saturating_sub(left).saturating_sub(right);
        let text = clip(text, available);
        format!("{}{}{}", " ".repeat(left), text, " ".repeat(right),)
    }
}
