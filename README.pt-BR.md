<p align="center"><img src="assets/logo.svg" alt="WinRust" width="160"></p>

# WinRust

**Um shell do Windows escrito em Rust: mais rápido e mais leve que o original.** Programas nativos pequenos (sem .NET, sem Electron) que substituem partes do Windows: a barra de tarefas, o Explorador de Arquivos, a pesquisa de arquivos, o Gerenciador de Tarefas, além de um limpador de disco, um mapa de disco e um visualizador de fotos. Um instalador coloca tudo, um botão tira tudo.

[English](README.md) | [Русский](README.ru.md) | [日本語](README.ja.md) | Português (Brasil)

<p align="center"><img src="assets/th.jpg" alt="WinRust" width="480"></p>

> **Leia isto primeiro.** O WinRust desativa a Pesquisa do Windows, oculta a barra de tarefas e pode desativar o Explorer como shell. O exe não tem assinatura de código, então o SmartScreen vai avisar. Um ponto de restauração é criado antes da instalação, mas teste primeiro em uma máquina reserva ou em uma VM.

## Instalação

1. Baixe o `WinRust.exe` da [última versão](https://github.com/Lemxy/WinRust/releases/latest) (Windows 10/11, x64).
2. Execute-o como administrador.
3. Marque os componentes que quiser e clique no botão de instalar. Em uma instalação nova, o `rust-explorer` vem desmarcado. Marque-o para substituir o shell Explorer do Windows (ou use a engrenagem na linha da topbar: o interruptor **Shell: rust-explorer**). Eles são instalados na ordem certa; um que falhe ou seja ignorado não impede os outros, e a janela lista o que deu errado.

A Pesquisa do Windows é opcional: em um Windows enxugado (por exemplo, uma imagem LTSC sem o serviço Pesquisa do Windows) essa etapa é ignorada com um aviso, em vez de falhar a instalação.

Se o UAC pedir a senha de **outro administrador**, as configurações por usuário (itens de inicialização, tarefas agendadas, perfil do PowerShell, permissões de pesquisa) ainda vão para o usuário que está conectado, não para o administrador que você digitou. Se o instalador não conseguir identificar quem é, ele para antes de alterar qualquer coisa.

**Atualizações automáticas:** após a primeira instalação, uma tarefa agendada `WinRust-update` (executa como SYSTEM, sem pedido do UAC) verifica a última versão na inicialização e uma vez por dia. Ela baixa apenas os programas que mudaram, confere cada arquivo com um manifesto assinado com a chave do projeto (uma versão adulterada ou sem assinatura é recusada) e substitui somente os arquivos que já estão instalados. Um programa em execução passa para a nova versão na próxima vez que iniciar (logon ou reinicialização). Log: `%ProgramData%\plugin-launcher\install.log` (linhas `autoupdate:`). Para desativar, desative a tarefa `WinRust-update` no Agendador de Tarefas; **Remover tudo** a exclui.

**Atualização manual:** baixe o novo `WinRust.exe` na página de versões, execute-o como administrador e selecione os componentes que quer atualizar ou instalar. Um `rust-explorer` existente continua selecionado por padrão nas atualizações. Suas configurações e os valores originais do Windows salvos permanecem.

O instalador abre em inglês. O botão de idioma no canto dele (ou a engrenagem da barra → **Configurações da barra → Idioma**) troca todos os programas WinRust para o idioma escolhido: English, Русский, 日本語 ou Português (Brasil). A escolha fica em `%ProgramData%\WinRust\lang.txt` (`en`, `ru`, `ja`, `pt`).

## Remoção

- **Remover tudo** na janela do instalador desfaz tudo: a Pesquisa do Windows, a barra de tarefas, o Explorer e os serviços voltam.
- Um plugin por vez: `plugin-launcher uninstall <plugin>` (veja [Linha de comando](#linha-de-comando)).
- **Se a tela ficar sem barra de tarefas ou área de trabalho:** pressione Ctrl+Alt+Del → Gerenciador de Tarefas → Executar nova tarefa → `explorer.exe`, depois execute o instalador e clique em Remover tudo.

Na desinstalação, o `rust-explorer` restaura os valores de inicialização dos serviços e o limite de agrupamento do svchost salvos antes da instalação. Instalações antigas sem esses backups mantêm os valores atuais e informam um aviso.

## O que é instalado

| Plugin | O que você ganha | O que muda no Windows |
|---|---|---|
| **rust-search** | Pesquisa de arquivos em todas as unidades NTFS, Alt+Espaço. Com a topbar, Win+R abre o modo Executar dele: histórico, preenchimento de pastas (Tab), Ctrl+Shift+Enter executa como administrador | Desativa o serviço Pesquisa do Windows, bloqueia o SearchHost.exe, oculta a caixa de pesquisa da barra de tarefas |
| **rtaskmgr-win32** | Gerenciador de Tarefas (Ctrl+Shift+Esc o abre) | O `taskmgr.exe` é redirecionado para ele (o arquivo do sistema não é tocado). Também instala o limpador de disco **rs-du**. A página **Velocidade** tem interruptores opcionais e reversíveis: perfis Jogo / Trabalho (param serviços em segundo plano, fecham lançadores ou apps de sincronização e os iniciam de novo), limpar o cache de espera da memória (agora, ou automaticamente enquanto a topbar roda), bloquear os processos do menu Iniciar, widgets e Phone Link do Windows, um `winget upgrade` semanal no lugar de atualizadores de terceiros, exclusões do Defender para caches de compilação e bibliotecas de jogos, NTFS mais rápido (sem horário do último acesso, sem nomes 8.3), um arquivo hosts que bloqueia anúncios. **Inicialização** também lista tarefas de logon/boot e serviços de início automático de terceiros, com o quanto cada um atrasou as últimas inicializações (o log de boot do próprio Windows). **O que está lento** mostra o tempo de drivers (DPC/interrupções), janelas travadas, disco lento e tempos de boot. Desinstalar reverte tudo |
| **topbar** | Uma barra inferior no estilo do Windows 11: janelas, relógio, volume, menus | Oculta a barra de tarefas original. Também instala o **reshell** (atalhos: Win+Ctrl+1/2/3 e Win+\` alternam áreas de trabalho (Win+1..9 abrem os botões de apps da barra), Win+D mostra a área de trabalho, Win+I abre as Configurações, Win+E o gerenciador de arquivos; traz a barra de tarefas de volta se a barra morrer), o **rs-du** e um bloco no seu perfil do PowerShell (`zoxide`, `grep`=rg, `ll`=eza, `cat`=bat; não faz nada se faltarem). O menu da engrenagem tem páginas curtas: Tema, Cores e transparência (qualquer cor da barra e das janelas da WinRust, a opacidade da barra), Modelos (salve suas próprias aparências e alterne entre elas), a barra (borda inferior / esquerda / direita, tamanho, ocultar automaticamente, botões na ponta esquerda como no Windows 10) e Restaurar tudo ao padrão |
| **rust-explorer** | Um gerenciador de arquivos com abas, e a área de trabalho | Pastas e unidades abrem nele; **o Explorer deixa de ser o shell** (volta sozinho se a topbar falhar). Em instalações novas, ajusta as configurações de inicialização de InventorySvc, lfsvc e DoSvc, e o limite de agrupamento do svchost, quando os valores podem ser salvos em backup. Copiador próprio (vários arquivos ao mesmo tempo em SSDs, sem buffer para arquivos grandes, progresso na linha de status) e, no botão direito, **Compactar em ZIP** (todos os núcleos) / **Extrair aqui** (ZIP e 7z), com nosso próprio motor de ZIP; um `.zip` abre como uma pasta (navegar, copiar para fora, abrir arquivos dele). "Mostrar na pasta" a partir de outros programas seleciona o arquivo (para as pastas comuns: Downloads, Documentos, Área de Trabalho, Imagens, Músicas, Vídeos e as fixadas, também quando nenhuma janela está aberta); os botões voltar / avançar do mouse funcionam; clique com o botão direito na área de trabalho para ocultar os ícones. Requer a topbar |
| **wiztree** | Um mapa de uso de disco ("WinRust Disk" no Iniciar) | Nada, além do arquivo e de um atalho |
| **photoview** | Um visualizador de fotos rápido | Adiciona-o a "Abrir com" (ele não vira o padrão) |
| **rust-settings** | Configurações em Rust: tela (resolução, taxa de atualização), som (volume), plano de energia e espaço nas unidades, volume por app e escolha de dispositivo, luz noturna, disposição das telas por arrastar, adaptadores de rede, Wi-Fi em qualquer adaptador (senha ou 802.1X), VPN e proxy, Bluetooth, apps, Windows Update, limpeza de disco, tema, sobre | Nada, além do arquivo e de um atalho. Win+I e os menus da barra abrem esta janela no lugar das Configurações do Windows (só algumas páginas raras ainda abrem as do sistema) |
| **debloat** | Desativa: telemetria, diagnósticos, Copilot, Recall, anúncios no Iniciar, Edge em segundo plano | Serviços e políticas de registro; os valores antigos são salvos e restaurados na desinstalação. **Somente linha de comando:** `install debloat --yes` |
| **purge** | Remove o lixo pré-instalado como o Tiny11 faz: Clipchamp, Notícias, Clima, sobreposições do Xbox, Teams, Cortana, Widgets, Skype e mais, além de tarefas de telemetria | **IRREVERSÍVEL, sem desfazer.** Os apps são removidos para todos os usuários e da imagem; a Store, as Configurações, o Iniciar e os runtimes ficam. Execute o `purge.exe` primeiro: ele só lista. **Somente linha de comando:** `install purge --yes` (OneDrive e Edge: `purge.exe --apply --onedrive --edge`) |
| chrome-rs | Um navegador | Ainda não instalável |

Toda alteração é reversível: desinstalar devolve os valores que existiam antes.

## Estilos

A engrenagem na ponta esquerda da barra abre as **Configurações da barra**; um estilo escolhido na página **Tema** vale na hora para a barra, o gerenciador de arquivos (rust-explorer) e a janela de pesquisa (Alt+Espaço):

| Estilo | Aparência |
|---|---|
| **Normal** | Azul do Windows 11, a barra embaixo |
| **Terminal** | Verde sobre preto, Consolas, cantos retos |
| **Dock flutuante** | Uma barra em forma de pílula no meio; janelas maximizadas ganham a tela toda, o dock se afasta por cima delas e volta na borda inferior |
| **Tiling** | Uma barra fina no topo, destaque âmbar, arredondada como a Normal; "Lado a lado" coloca as janelas abertas lado a lado |
| **Estrada branca** | Marfim claro com dourado: triângulos low-poly suaves, alguns contornados em dourado, atrás da barra, de seus painéis e do rust-settings |
| **Rubi** | Granada profunda com destaque rubi brilhante: um degradê suave de cima para baixo com um leve brilho, atrás da barra e de seus painéis |
| **Windows 98** (BETA) | Cinza clássico com destaque azul-marinho, cantos retos, Tahoma. Por enquanto só cores, cantos e fonte: ainda sem bordas 3D |
| **Windows 7** (BETA) | Azul vítreo no estilo Aero, texto branco |

O menu da engrenagem também define a posição da barra (embaixo, à esquerda ou à direita: uma barra lateral mostra só ícones e os painéis abrem ao lado dela) e o ocultar automático.

A escolha fica em `%ProgramData%\WinRust\style.txt`.

## Benchmark

Medições pontuais em uma única máquina (Windows 11 IoT Enterprise 10.0.26300, RTX 4060 Laptop, NTFS). Trate-as como ordens de grandeza, não como números de laboratório.

| O quê | Windows | WinRust | Diferença |
|---|---|---|---|
| Pesquisa de arquivos (rust-search em todo o C: vs `dir /s` em uma pasta) | 8.340 ms | 45 ms | **99,5%** mais rápido |
| Igual, contra `Get-ChildItem -Recurse` | 16.200 – 28.600 ms | 45 ms | **99,7–99,8%** mais rápido |
| Abrir uma janela de pasta (rust-explorer, mediana de 5 execuções) | 420 ms | 60 ms | **86%** mais rápido |
| Memória do shell (topbar + rust-explorer + rs-shell + reshell vs explorer.exe) | 366 MB | 46 MB | **87%** menos |
| Igual, mais o índice de pesquisa (53 MB) | 366 MB | 99 MB | **73%** menos |
| Tamanho do binário do gerenciador de arquivos (agora com ZIP e 7z dentro) | 3.280 KB | 1.179 KB | **64%** menor |
| Tamanho do binário do gerenciador de tarefas | 5.791 KB | 954 KB | **84%** menor |
| CPU ociosa da barra (topbar após otimização, % de um núcleo) | 0,00–0,10% (explorer) | 0,05–0,10% | no mesmo nível do Explorer |
| Copiar 5.000 arquivos pequenos, ~160 MB (rust-explorer: IFileOperation, o motor do Explorer, vs copiador próprio) | 16,3 s | 2,3 s | **86%** mais rápido (7x) |
| Copiar um arquivo de 1 GB (idem) | 0,58 s | 0,60 s | igual |
| Compactar os mesmos 5.000 arquivos em ZIP (`Compress-Archive` vs Compactar em ZIP do rust-explorer) | 48,4 s | 0,75 s | **98%** mais rápido (64x) |

Resumindo: **64–99% mais rápido ou menor nos testes de tamanho/velocidade, igual na CPU ociosa e na cópia de um arquivo grande.** Ressalvas: o rs pesquisa um índice MFT pronto (e paga 53 MB de memória por isso), enquanto `dir` e PowerShell percorrem o disco do zero; o explorer.exe continuava rodando com suas janelas durante o teste, então o ganho de memória está um pouco exagerado; a Pesquisa do Windows e o Gerenciador de Tarefas original não foram comparados. As execuções de cópia e ZIP (`cargo test -p rust-explorer --release bench -- --ignored`) foram únicas, com dados recém-gravados, então os dois lados leram do cache de memória; nosso ZIP saiu um pouco maior (0,9 MB contra 0,7 MB) porque cada arquivo é compactado em seu próprio núcleo.

## Capturas de tela

**Barra de tarefas**

![Barra de tarefas na área de trabalho](docs/screenshots/taskbar.webp)
![Menu da barra de tarefas](docs/screenshots/taskbar-menu.png)
![Menu da barra, nova aparência](docs/screenshots/topbar-menu-new.png)

**Gerenciador de Tarefas** (interface em russo)

![Gerenciador de Tarefas](docs/screenshots/task-manager.webp)

**Gerenciador de arquivos**

![Gerenciador de arquivos](docs/screenshots/explorer.png)

---

## Para desenvolvedores

### Estrutura
```
src/            o instalador (plugin-launcher.exe)
  main.rs         a tabela de plugins PLUGINS: nome, textos, par instalar/desinstalar, ícone
  install/        o que cada plugin altera e como é desfeito (um arquivo por plugin)
  cli.rs, ui.rs   a linha de comando e a janela; ambos chamam as mesmas funções de instalação
common/         winrust-common: troca de idioma (t!/tf!), estilos e paletas da interface, analisador da saída do reg.exe, leitor da MFT do NTFS
plugins/        fontes de cada plugin, cada um em seu próprio crate
ReShell/        reshell (atalhos da área de trabalho, rede de segurança da topbar), vem com a topbar
payload.txt     os exes embutidos no instalador e onde a compilação os coloca
payload/        esses exes depois de compilados (não estão no git)
scripts/        compilação, atualização, versão
```
Tudo é um único workspace do cargo (um `Cargo.lock`, todos os exes em `target\release`). `plugins/rust-search` e `plugins/chrome-rs` são compilados separadamente porque precisam de `panic = "unwind"` (veja `Cargo.toml`).

### Adicionar um idioma
`t!("русский", "english", ja: "日本語", pt: "português")` e `tf!(…)` devolvem cada texto no idioma atual (sem `ja:` e `pt:`, japonês e português mostram o inglês). Os idiomas são o `Lang` em `common/src/lib.rs`.

### Compilação
Requer Windows x64, Rust stable (1.85+) com a toolchain MSVC.
```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-all.ps1
```
Resultado: `target\release\plugin-launcher.exe`, com todos os plugins dentro. Feche antes um instalador em execução. Testes: `cargo test --workspace` (e `cargo test` em `plugins\rust-search`). O CI roda o mesmo a cada push (`.github/workflows/build.yml`).

| Script | O que faz |
|---|---|
| `build-all.ps1` | tudo: plugins → `payload\` → instalador |
| `build-plugins.ps1` | só os plugins → `payload\` (sem recompilar o instalador) |
| `update-plugin.ps1 <plugin>` | recompila e reinstala um plugin instalado sem recompilar o instalador (pede administrador pelo UAC) |
| `user-mode.ps1`, `dev-update.ps1` | modo desenvolvedor: roda topbar, rust-explorer, rs-shell e reshell a partir de `%LOCALAPPDATA%` sobre o Explorer e os troca sem administrador |
| `bench.ps1` | o benchmark da pesquisa e do Gerenciador de Tarefas |
| `publish.ps1`, `release.ps1` | commit e push; versão no GitHub com o `WinRust.exe` |

### Linha de comando
De um terminal **com direitos de administrador**:
```
plugin-launcher list
plugin-launcher install <plugin>... --yes (um ou mais nomes, ou `all` para os componentes padrão; sem --yes só mostra avisos)
plugin-launcher update <plugin>|all        (reinstala a partir de payload\ ao lado do exe)
plugin-launcher uninstall <plugin>
plugin-launcher rollback --yes           (remove tudo)
plugin-launcher autoupdate               (uma verificação de atualização agora: o que a tarefa WinRust-update executa)
```

Para incluir o `rust-explorer` em uma instalação em massa nova, execute `plugin-launcher install all rust-explorer --yes`. Se a `topbar` já estiver instalada, você pode instalar o `rust-explorer` sozinho.
Vários plugins em um comando são instalados na ordem da tabela (topbar antes do rust-explorer); um que falhe não impede os demais. Códigos de saída: 0 sucesso, 1 erro (pelo menos um plugin falhou), 2 argumentos inválidos ou falta de `--yes`. Sem argumentos, a janela abre. Log de instalação: `%ProgramData%\plugin-launcher\install.log`.

### Adicionar um plugin
1. Um crate em `plugins/<nome>`, adicionado a `members` no `Cargo.toml` da raiz.
2. Uma linha em `payload.txt`.
3. `src/install/<nome>.rs` com um par instalar/desinstalar (e uma linha `mod` em `src/install/mod.rs`). Salve os valores originais antes de alterar qualquer coisa, para que a desinstalação possa devolvê-los.
4. Uma entrada em `PLUGINS` (`src/main.rs`). A janela, a altura dela e a linha de comando a reconhecem sozinhas.

## Licença

Licenciado sob [MIT](LICENSE-MIT) ou [Apache-2.0](LICENSE-APACHE), à sua escolha.
