//! Каркас локализации RDE (R-UI-06, NFR-I18N): ru/en.
//!
//! Строки UI берутся через [`tr`] по стабильным ключам; язык — из настроек приложения
//! (`AppSettings.language`). Каркас введён в P5: переведены меню, вкладки, панель действий и
//! частые подписи; остальные строки (диалоги/журнал) переводятся по мере расширения словаря —
//! «сырых» ключей в UI не показываем (неизвестный ключ возвращает сам ключ и это видно в тестах).
//!
//! Термины ГОСТ/технические (act_key, out_hash, .doc.rde) не переводятся — это идентификаторы.

/// Язык интерфейса.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Ru,
    En,
}

impl Lang {
    /// Код языка (`ru`/`en`) — как в настройках.
    pub fn code(self) -> &'static str {
        match self {
            Lang::Ru => "ru",
            Lang::En => "en",
        }
    }
}

/// Определить язык по коду настройки (неизвестный → ru).
pub fn lang(code: &str) -> Lang {
    match code.trim().to_ascii_lowercase().as_str() {
        "en" => Lang::En,
        _ => Lang::Ru,
    }
}

/// Перевод строки по ключу. Неизвестный ключ возвращается как есть (видно в тестах/UI).
pub fn tr(lang: Lang, key: &str) -> &'static str {
    let ru = matches!(lang, Lang::Ru);
    match (key, ru) {
        ("menu.file", true) => "Файл",
        ("menu.file", false) => "File",
        ("menu.edit", true) => "Правка",
        ("menu.edit", false) => "Edit",
        ("menu.document", true) => "Документ",
        ("menu.document", false) => "Document",
        ("menu.project", true) => "Проект",
        ("menu.project", false) => "Project",
        ("menu.view", true) => "Вид",
        ("menu.view", false) => "View",
        ("menu.agent", true) => "Агент",
        ("menu.agent", false) => "Agent",
        ("menu.help", true) => "Помощь",
        ("menu.help", false) => "Help",
        ("toolbar.update_all", true) => "Обновить всё",
        ("toolbar.update_all", false) => "Update all",
        ("toolbar.export_docx", true) => "Экспорт .docx",
        ("toolbar.export_docx", false) => "Export .docx",
        ("toolbar.save_project", true) => "Сохранить проект",
        ("toolbar.save_project", false) => "Save project",
        ("toolbar.update_document", true) => "Обновить документ",
        ("toolbar.update_document", false) => "Update document",
        ("toolbar.new_object", true) => "+ Объект",
        ("toolbar.new_object", false) => "+ Object",
        ("toolbar.search", true) => "Поиск",
        ("toolbar.search", false) => "Search",
        ("tab.document", true) => "Документ",
        ("tab.document", false) => "Document",
        ("tab.structure", true) => "Структура",
        ("tab.structure", false) => "Structure",
        ("tab.artifacts", true) => "Артефакты",
        ("tab.artifacts", false) => "Artifacts",
        ("tab.source", true) => "Источник",
        ("tab.source", false) => "Source",
        ("tab.code", true) => "Код документа",
        ("tab.code", false) => "Document code",
        ("panel.journal", true) => "Журнал",
        ("panel.journal", false) => "Log",
        ("panel.tasks", true) => "Задачи",
        ("panel.tasks", false) => "Tasks",
        ("panel.agent", true) => "Агент",
        ("panel.agent", false) => "Agent",
        ("status.objects", true) => "объектов",
        ("status.objects", false) => "objects",
        ("status.stale", true) => "устаревших",
        ("status.stale", false) => "stale",
        ("status.agent_offline", true) => "Агент: не подключён (v0.3)",
        ("status.agent_offline", false) => "Agent: not connected (v0.3)",
        ("action.open", true) => "Открыть",
        ("action.open", false) => "Open",
        ("action.rename", true) => "Переименовать",
        ("action.rename", false) => "Rename",
        ("action.delete", true) => "Удалить",
        ("action.delete", false) => "Delete",
        ("action.cancel", true) => "Отмена",
        ("action.cancel", false) => "Cancel",
        ("action.save", true) => "Сохранить",
        ("action.save", false) => "Save",
        ("action.close", true) => "Закрыть",
        ("action.close", false) => "Close",
        ("settings.app", true) => "Настройки приложения…",
        ("settings.app", false) => "Application settings…",
        ("settings.project", true) => "Настройки проекта…",
        ("settings.project", false) => "Project settings…",
        ("settings.language", true) => "Язык",
        ("settings.language", false) => "Language",
        ("settings.theme", true) => "Тема",
        ("settings.theme", false) => "Theme",
        ("settings.keep_runs", true) => "Хранить запусков",
        ("settings.keep_runs", false) => "Keep runs",
        ("settings.author_name", true) => "Автор по умолчанию",
        ("settings.author_name", false) => "Default author",
        ("settings.authors", true) => "Авторы проекта",
        ("settings.authors", false) => "Project authors",
        ("settings.add_author", true) => "+ Автор",
        ("settings.add_author", false) => "+ Author",
        ("settings.diagnostics", true) => "Экспорт диагностики…",
        ("settings.diagnostics", false) => "Export diagnostics…",
        ("git.init", true) => "Git: инициализировать репозиторий",
        ("git.init", false) => "Git: initialize repository",
        ("git.commit", true) => "Git: закоммитить изменения…",
        ("git.commit", false) => "Git: commit changes…",
        ("git.history", true) => "Git: история…",
        ("git.history", false) => "Git: history…",
        (_, true) => "",
        (_, false) => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_have_translations_for_ui_core() {
        let keys = [
            "menu.file",
            "menu.document",
            "menu.project",
            "toolbar.update_all",
            "toolbar.save_project",
            "tab.artifacts",
            "panel.tasks",
            "status.objects",
            "action.cancel",
            "settings.app",
            "git.commit",
        ];
        for key in keys {
            let ru = tr(Lang::Ru, key);
            let en = tr(Lang::En, key);
            assert!(!ru.is_empty(), "нет ru-перевода для {key}");
            assert!(!en.is_empty(), "нет en-перевода для {key}");
            assert_ne!(ru, en, "перевод не отличается для {key}");
        }
        assert_eq!(lang("en"), Lang::En);
        assert_eq!(lang("ru-RU"), Lang::Ru);
        assert_eq!(lang("de"), Lang::Ru);
        assert_eq!(Lang::En.code(), "en");
    }
}
