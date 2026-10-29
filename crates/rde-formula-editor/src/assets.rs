//! Встроенные ассеты окна редактора формул (P6, ADR-0022 §3.2): страница MathLive и её шрифты
//! упакованы в бинарник (`include_bytes!`), поэтому рантайм **полностью офлайновый** — нет CDN,
//! сетевых загрузок и внешних файлов (кроме самого браузера, который запускает RDE).
//!
//! Лицензии: MathLive — MIT (Copyright (c) 2017 – present Arno Gourdol); шрифты KaTeX — MIT
//! (KaTeX authors / Khan Academy). Тексты лицензий лежат рядом и дублируются в `NOTICE`.
//! Ассеты обновляются только вместе с версией (пин `mathlive 0.111.1`) и фиксируются тестом
//! [`tests::asset_inventory_is_bundled_and_licensed`].

/// Один встроенный ассет: URL на loopback-сервере моста, MIME-тип и содержимое.
pub struct Asset {
    /// Путь на сервере моста (например, `/mathlive/mathlive.min.js`).
    pub url: &'static str,
    /// MIME-тип для заголовка `Content-Type`.
    pub mime: &'static str,
    /// Содержимое файла (встроено в бинарник).
    pub bytes: &'static [u8],
    /// Путь в репозитории — для документации/NOTICE и тестов инвентаризации.
    pub file: &'static str,
}

macro_rules! asset {
    ($url:literal, $mime:literal, $file:literal) => {
        Asset {
            url: $url,
            mime: $mime,
            bytes: include_bytes!(concat!("../assets/", $file)),
            file: concat!("assets/", $file),
        }
    };
}

/// Все ассеты окна: страница редактора, MathLive (MIT) и шрифты KaTeX (MIT).
pub static ASSETS: &[Asset] = &[
    asset!("/editor/editor.html", "text/html; charset=utf-8", "editor/editor.html"),
    asset!("/editor/editor.js", "text/javascript; charset=utf-8", "editor/editor.js"),
    asset!("/editor/editor.css", "text/css; charset=utf-8", "editor/editor.css"),
    asset!("/mathlive/mathlive.min.js", "text/javascript; charset=utf-8", "mathlive/mathlive.min.js"),
    asset!("/mathlive/mathlive-static.css", "text/css; charset=utf-8", "mathlive/mathlive-static.css"),
    asset!("/mathlive/mathlive-fonts.css", "text/css; charset=utf-8", "mathlive/mathlive-fonts.css"),
    asset!("/mathlive/fonts/KaTeX_AMS-Regular.woff2", "font/woff2", "mathlive/fonts/KaTeX_AMS-Regular.woff2"),
    asset!("/mathlive/fonts/KaTeX_Caligraphic-Bold.woff2", "font/woff2", "mathlive/fonts/KaTeX_Caligraphic-Bold.woff2"),
    asset!("/mathlive/fonts/KaTeX_Caligraphic-Regular.woff2", "font/woff2", "mathlive/fonts/KaTeX_Caligraphic-Regular.woff2"),
    asset!("/mathlive/fonts/KaTeX_Fraktur-Bold.woff2", "font/woff2", "mathlive/fonts/KaTeX_Fraktur-Bold.woff2"),
    asset!("/mathlive/fonts/KaTeX_Fraktur-Regular.woff2", "font/woff2", "mathlive/fonts/KaTeX_Fraktur-Regular.woff2"),
    asset!("/mathlive/fonts/KaTeX_Main-Bold.woff2", "font/woff2", "mathlive/fonts/KaTeX_Main-Bold.woff2"),
    asset!("/mathlive/fonts/KaTeX_Main-BoldItalic.woff2", "font/woff2", "mathlive/fonts/KaTeX_Main-BoldItalic.woff2"),
    asset!("/mathlive/fonts/KaTeX_Main-Italic.woff2", "font/woff2", "mathlive/fonts/KaTeX_Main-Italic.woff2"),
    asset!("/mathlive/fonts/KaTeX_Main-Regular.woff2", "font/woff2", "mathlive/fonts/KaTeX_Main-Regular.woff2"),
    asset!("/mathlive/fonts/KaTeX_Math-BoldItalic.woff2", "font/woff2", "mathlive/fonts/KaTeX_Math-BoldItalic.woff2"),
    asset!("/mathlive/fonts/KaTeX_Math-Italic.woff2", "font/woff2", "mathlive/fonts/KaTeX_Math-Italic.woff2"),
    asset!("/mathlive/fonts/KaTeX_SansSerif-Bold.woff2", "font/woff2", "mathlive/fonts/KaTeX_SansSerif-Bold.woff2"),
    asset!("/mathlive/fonts/KaTeX_SansSerif-Italic.woff2", "font/woff2", "mathlive/fonts/KaTeX_SansSerif-Italic.woff2"),
    asset!("/mathlive/fonts/KaTeX_SansSerif-Regular.woff2", "font/woff2", "mathlive/fonts/KaTeX_SansSerif-Regular.woff2"),
    asset!("/mathlive/fonts/KaTeX_Script-Regular.woff2", "font/woff2", "mathlive/fonts/KaTeX_Script-Regular.woff2"),
    asset!("/mathlive/fonts/KaTeX_Size1-Regular.woff2", "font/woff2", "mathlive/fonts/KaTeX_Size1-Regular.woff2"),
    asset!("/mathlive/fonts/KaTeX_Size2-Regular.woff2", "font/woff2", "mathlive/fonts/KaTeX_Size2-Regular.woff2"),
    asset!("/mathlive/fonts/KaTeX_Size3-Regular.woff2", "font/woff2", "mathlive/fonts/KaTeX_Size3-Regular.woff2"),
    asset!("/mathlive/fonts/KaTeX_Size4-Regular.woff2", "font/woff2", "mathlive/fonts/KaTeX_Size4-Regular.woff2"),
    asset!("/mathlive/fonts/KaTeX_Typewriter-Regular.woff2", "font/woff2", "mathlive/fonts/KaTeX_Typewriter-Regular.woff2"),
];

/// Найти ассет по URL-пути моста (`/mathlive/...`, `/editor/...`).
#[must_use]
pub fn lookup(url: &str) -> Option<&'static Asset> {
    ASSETS.iter().find(|asset| asset.url == url)
}

/// Суммарный размер встроенных ассетов в байтах (для отчёта/NOTICE).
#[must_use]
pub fn total_bytes() -> usize {
    ASSETS.iter().map(|asset| asset.bytes.len()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Инвентаризация: все заявленные файлы встроены, непустые, MIME корректен, лицензии
    /// MathLive/KaTeX лежат рядом (NFR-LIC-01/02: атрибуция в бандле).
    #[test]
    fn asset_inventory_is_bundled_and_licensed() {
        assert!(ASSETS.len() >= 25, "ассеты страницы+MathLive+шрифты: {}", ASSETS.len());
        for asset in ASSETS {
            assert!(!asset.bytes.is_empty(), "пустой ассет: {}", asset.file);
            assert!(asset.url.starts_with('/'), "URL ассета: {}", asset.url);
        }
        let mathlive = lookup("/mathlive/mathlive.min.js").expect("MathLive");
        assert!(mathlive.bytes.len() > 500_000, "MathLive-бандл целиком: {}", mathlive.bytes.len());
        assert_eq!(mathlive.mime, "text/javascript; charset=utf-8");
        assert!(lookup("/mathlive/fonts/KaTeX_Main-Regular.woff2").is_some(), "шрифт KaTeX в бандле");
        assert!(lookup("/editor/editor.html").is_some());
        let total = total_bytes();
        assert!(total > 1_000_000, "суммарный размер ассетов: {total}");
        // Лицензии лежат в репозитории рядом с ассетами (проверка файлов на диске).
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/licenses");
        let mathlive_license = std::fs::read_to_string(root.join("LICENSE-mathlive.txt")).expect("лицензия MathLive");
        assert!(mathlive_license.contains("Permission is hereby granted"), "MIT-текст MathLive");
        let katex_license = std::fs::read_to_string(root.join("LICENSE-katex-fonts.txt")).expect("лицензия KaTeX");
        assert!(katex_license.to_lowercase().contains("mit"), "MIT-текст шрифтов KaTeX");
    }

    /// Страница ссылается только на встроенные ассеты (офлайн: ни одного внешнего URL).
    #[test]
    fn page_references_only_bundled_local_assets() {
        let html = std::str::from_utf8(lookup("/editor/editor.html").expect("html").bytes).expect("utf8");
        for needle in [
            "./editor/editor.css",
            "./editor/editor.js",
            "./mathlive/mathlive-static.css",
            "./mathlive/mathlive-fonts.css",
            "./mathlive/mathlive.min.js",
        ] {
            let relative = needle.trim_start_matches("./");
            assert!(html.contains(needle), "страница подключает {needle}");
            assert!(lookup(&format!("/{relative}")).is_some(), "ассет {relative} в бандле");
        }
        assert!(!html.contains("http://"), "внешние URL в странице запрещены");
        assert!(!html.contains("https://"), "внешние URL в странице запрещены");
    }
}
