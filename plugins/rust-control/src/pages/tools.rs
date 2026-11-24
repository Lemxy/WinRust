//! Shortcuts to the administrative consoles Windows ships.

use crate::model::{Action, Row, Target, Tone};

fn tool(title: &str, subtitle: &str, file: &'static str) -> Row {
    Row::entry(title, subtitle, file, t!("Открыть", "Open", ja: "開く", pt: "Abrir", zh: "打开").to_string(), Tone::Neutral, Some(Action::Open(Target::File(file))), vec![])
}

pub fn rows() -> Vec<Row> {
    vec![
        Row::note(
            t!("Консоли MMC запускаются от имени администратора, если Windows попросит.", "MMC consoles start elevated when Windows asks for it.", ja: "MMC コンソールは、Windows から求められた場合に管理者として起動します。", pt: "Os consoles MMC são iniciados como administrador quando o Windows solicita.", zh: "如果 Windows 要求，MMC 控制台将以管理员身份启动。"),
            Tone::Dim,
        ),
        Row::section(t!("Управление компьютером", "Computer Management", ja: "コンピューターの管理", pt: "Gerenciamento do computador", zh: "计算机管理")),
        tool(
            t!("Управление компьютером", "Computer Management", ja: "コンピューターの管理", pt: "Gerenciamento do computador", zh: "计算机管理"),
            "compmgmt.msc",
            "compmgmt.msc",
        ),
        tool(
            t!("Управление дисками", "Disk Management", ja: "ディスクの管理", pt: "Gerenciamento de disco", zh: "磁盘管理"),
            "diskmgmt.msc",
            "diskmgmt.msc",
        ),
        tool(
            t!("Просмотр событий", "Event Viewer", ja: "イベント ビューアー", pt: "Visualizador de eventos", zh: "事件查看器"),
            "eventvwr.msc",
            "eventvwr.msc",
        ),
        tool(t!("Службы", "Services", ja: "サービス", pt: "Serviços", zh: "服务"), "services.msc", "services.msc"),
        tool(
            t!("Диспетчер устройств", "Device Manager", ja: "デバイス マネージャー", pt: "Gerenciador de Dispositivos", zh: "设备管理器"),
            "devmgmt.msc",
            "devmgmt.msc",
        ),
        tool(
            t!("Планировщик заданий", "Task Scheduler", ja: "タスク スケジューラ", pt: "Agendador de tarefas", zh: "任务计划程序"),
            "taskschd.msc",
            "taskschd.msc",
        ),
        Row::section(t!("Редакторы и сведения", "Editors and information", ja: "エディターと情報", pt: "Editores e informações", zh: "编辑器和信息")),
        tool(
            t!("Редактор реестра", "Registry Editor", ja: "レジストリ エディター", pt: "Editor do Registro", zh: "注册表编辑器"),
            "regedit.exe",
            "regedit.exe",
        ),
        tool(
            t!("Сведения о системе", "System Information", ja: "システム情報", pt: "Informações do sistema", zh: "系统信息"),
            "msinfo32.exe",
            "msinfo32.exe",
        ),
        Row::entry(
            t!("Все средства администрирования", "All administrative tools", ja: "すべての管理ツール", pt: "Todas as ferramentas administrativas", zh: "所有管理工具"),
            "control.exe /name Microsoft.AdministrativeTools",
            "",
            t!("Открыть", "Open", ja: "開く", pt: "Abrir", zh: "打开"),
            Tone::Neutral,
            Some(Action::Open(Target::Control("Microsoft.AdministrativeTools"))),
            vec![],
        ),
    ]
}
