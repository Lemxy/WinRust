//! Page registry: one function per page builds the rows it can show right now.

pub mod accessibility;
pub mod datetime;
pub mod devices;
pub mod home;
pub mod network;
pub mod power;
pub mod programs;
pub mod security;
pub mod services;
pub mod system;
pub mod tools;
pub mod users;

use crate::model::{PageId, PageMeta, Row};
use winrust_common::style::rgb;

/// Title, icon, tile colour and search words of every page, in navigation order.
pub fn meta() -> Vec<PageMeta> {
    let page = |id: PageId, title: String, description: String, icon: &'static str, tile: u32, keywords: &str| PageMeta {
        id,
        title,
        description,
        icon,
        tile,
        keywords: keywords.to_string(),
    };
    vec![
        page(
            PageId::Home,
            t!("Главная", "Home", ja: "ホーム", pt: "Início", zh: "主页").into(),
            t!("Категории классической панели управления", "Classic Control Panel categories", ja: "従来のコントロール パネルのカテゴリ", pt: "Categorias clássicas do Painel de Controle", zh: "经典控制面板类别").into(),
            "\u{E80F}",
            rgb(10, 132, 255),
            "home inicio main categorias categories control panel panel de control",
        ),
        page(
            PageId::System,
            t!("Система", "System", ja: "システム", pt: "Sistema", zh: "系统").into(),
            t!("Выпуск Windows, оборудование и среда", "Windows edition, hardware and environment", ja: "Windows のエディション、ハードウェア、環境", pt: "Edição do Windows, hardware e ambiente", zh: "Windows 版本、硬件和环境").into(),
            "\u{E7F4}",
            rgb(10, 132, 255),
            "system sistema systeminfo pc computer about windows edition build cpu ram gpu environment",
        ),
        page(
            PageId::Programs,
            t!("Программы", "Programs", ja: "プログラム", pt: "Programas", zh: "程序").into(),
            t!("Установленное программное обеспечение", "Installed software", ja: "インストールされているソフトウェア", pt: "Software instalado", zh: "已安装的软件").into(),
            "\u{E71D}",
            rgb(255, 159, 10),
            "programs programas apps software uninstall remove installed",
        ),
        page(
            PageId::Services,
            t!("Службы", "Services", ja: "サービス", pt: "Serviços", zh: "服务").into(),
            t!("Запуск, остановка и тип запуска служб Windows", "Start, stop and configure Windows services", ja: "Windows サービスの開始、停止、構成", pt: "Iniciar, parar e configurar serviços do Windows", zh: "启动、停止和配置 Windows 服务").into(),
            "\u{E9F5}",
            rgb(191, 90, 242),
            "services servicios windows service start stop automatic manual disabled scm",
        ),
        page(
            PageId::Devices,
            t!("Устройства", "Devices", ja: "デバイス", pt: "Dispositivos", zh: "设备").into(),
            t!("Устройства, которые Windows видит сейчас", "Devices Windows recognises right now", ja: "現在 Windows が認識しているデバイス", pt: "Dispositivos que o Windows reconhece agora", zh: "Windows 当前识别的设备").into(),
            "\u{E772}",
            rgb(50, 173, 230),
            "devices dispositivos hardware pnp device manager driver printer",
        ),
        page(
            PageId::Network,
            t!("Сеть", "Network", ja: "ネットワーク", pt: "Rede", zh: "网络").into(),
            t!("Адаптеры, адреса, шлюзы и DNS", "Adapters, addresses, gateways and DNS", ja: "アダプター、アドレス、ゲートウェイ、DNS", pt: "Adaptadores, endereços, gateways e DNS", zh: "适配器、地址、网关和 DNS").into(),
            "\u{E774}",
            rgb(0, 122, 255),
            "network red internet adapter ethernet wifi ipv4 ipv6 dns gateway mac",
        ),
        page(
            PageId::Users,
            t!("Учётные записи", "User Accounts", ja: "ユーザー アカウント", pt: "Contas de usuário", zh: "用户账户").into(),
            t!("Локальные пользователи и группы", "Local accounts and groups", ja: "ローカル ユーザーとグループ", pt: "Contas e grupos locais", zh: "本地用户和组").into(),
            "\u{E77B}",
            rgb(52, 199, 89),
            "users usuarios accounts cuentas groups grupos local netuser",
        ),
        page(
            PageId::Power,
            t!("Электропитание", "Power", ja: "電源", pt: "Energia", zh: "电源").into(),
            t!("Схемы питания и состояние батареи", "Power plans and battery status", ja: "電源プランとバッテリーの状態", pt: "Planos de energia e bateria", zh: "电源计划和电池状态").into(),
            "\u{E7E8}",
            rgb(48, 209, 88),
            "power energia battery bateria plan scheme balanced high performance",
        ),
        page(
            PageId::DateTime,
            t!("Дата и время", "Date and Time", ja: "日付と時刻", pt: "Data e hora", zh: "日期和时间").into(),
            t!("Часовой пояс, дата и время", "Time zone, date and time", ja: "タイム ゾーン、日付と時刻", pt: "Fuso horário, data e hora", zh: "时区、日期和时间").into(),
            "\u{E823}",
            rgb(94, 92, 230),
            "date fecha time hora clock reloj timezone zone zona timedate cpl",
        ),
        page(
            PageId::Accessibility,
            t!("Специальные возможности", "Accessibility", ja: "ユーザー補助", pt: "Acessibilidade", zh: "辅助功能").into(),
            t!("Лупа, контраст и клавиатурные помощники", "Magnifier, high contrast and keyboard aids", ja: "拡大鏡、ハイ コントラスト、キーボード アシスト", pt: "Lupa, alto contraste e auxílios de teclado", zh: "放大镜、高对比度和键盘辅助").into(),
            "\u{E776}",
            rgb(50, 173, 230),
            "accessibility accesibilidad ease of access magnifier lupa contrast sticky keys",
        ),
        page(
            PageId::Security,
            t!("Безопасность", "Security", ja: "セキュリティ", pt: "Segurança", zh: "安全").into(),
            t!("Состояние Защитника Windows и брандмауэра", "Defender and firewall status", ja: "Defender とファイアウォールの状態", pt: "Status do Defender e do firewall", zh: "Defender 和防火墙状态").into(),
            "\u{E72E}",
            rgb(255, 69, 58),
            "security seguridad defender antivirus firewall firewall real time protection",
        ),
        page(
            PageId::Tools,
            t!("Администрирование", "Administrative Tools", ja: "管理ツール", pt: "Ferramentas administrativas", zh: "管理工具").into(),
            t!("Консоли и редакторы Windows", "Windows consoles and editors", ja: "Windows のコンソールとエディター", pt: "Consoles e editores do Windows", zh: "Windows 控制台和编辑器").into(),
            "\u{E90F}",
            rgb(142, 142, 147),
            "tools herramientas administrative admin mmc event viewer services registry disk management task scheduler",
        ),
    ]
}

/// Reads a page fresh. Called on navigation and by the refresh timer.
pub fn load(id: PageId) -> Vec<Row> {
    match id {
        PageId::Home => vec![],
        PageId::System => system::rows(),
        PageId::Programs => programs::rows(),
        PageId::Services => services::rows(),
        PageId::Devices => devices::rows(),
        PageId::Network => network::rows(),
        PageId::Users => users::rows(),
        PageId::Power => power::rows(),
        PageId::DateTime => datetime::rows(),
        PageId::Accessibility => accessibility::rows(),
        PageId::Security => security::rows(),
        PageId::Tools => tools::rows(),
    }
}
