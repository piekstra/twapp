use rand::Rng;

pub struct ThemeColor {
    pub background: &'static str,
    pub name: &'static str,
}

pub const THEME_PALETTE: &[ThemeColor] = &[
    ThemeColor { background: "#f5f5f7", name: "default" },      // Neutral gray (never randomly picked)
    ThemeColor { background: "#ffe0e0", name: "rose" },         // Warm pink
    ThemeColor { background: "#e0e8ff", name: "cornflower" },    // Soft blue
    ThemeColor { background: "#e0ffe0", name: "mint" },         // Fresh green
    ThemeColor { background: "#fff0e0", name: "peach" },        // Light orange
    ThemeColor { background: "#f0e0ff", name: "lavender" },     // Soft purple
    ThemeColor { background: "#e0ffff", name: "seafoam" },       // Light teal
    ThemeColor { background: "#fef3c7", name: "lemon" },        // Bright yellow
    ThemeColor { background: "#e8d8cc", name: "cappuccino" },   // Warm tan
    ThemeColor { background: "#e8f0e0", name: "sage" },         // Muted green
];

/// Pick a random color from the palette, excluding the default (index 0)
pub fn random_color() -> &'static str {
    let idx = rand::rng().random_range(1..THEME_PALETTE.len());
    THEME_PALETTE[idx].background
}
