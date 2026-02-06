use rand::Rng;

pub struct ThemeColor {
    pub background: &'static str,
    pub name: &'static str,
}

pub const THEME_PALETTE: &[ThemeColor] = &[
    ThemeColor { background: "#f5f5f7", name: "default" },   // Neutral gray
    ThemeColor { background: "#ffe0e0", name: "rose" },      // Light red
    ThemeColor { background: "#e0e8ff", name: "sky" },       // Light blue
    ThemeColor { background: "#e0ffe0", name: "mint" },      // Light green
    ThemeColor { background: "#fff0e0", name: "peach" },     // Light orange
    ThemeColor { background: "#f0e0ff", name: "lavender" },  // Light purple
    ThemeColor { background: "#e0ffff", name: "aqua" },      // Light teal
    ThemeColor { background: "#fff5e0", name: "cream" },     // Light yellow
    ThemeColor { background: "#ffe0f0", name: "blush" },     // Light pink
    ThemeColor { background: "#e8f0e0", name: "sage" },      // Light sage
];

/// Pick a random color from the palette, excluding the default (index 0)
pub fn random_color() -> &'static str {
    let idx = rand::rng().random_range(1..THEME_PALETTE.len());
    THEME_PALETTE[idx].background
}
