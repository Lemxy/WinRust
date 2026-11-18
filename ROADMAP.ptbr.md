# WinRust: Roadmap

English version: [ROADMAP.md](ROADMAP.md) | Русская версия: [ROADMAP.ru.md](ROADMAP.ru.md)

Cada item indica o nível de verificação realizado: **tests** (testes unitários), **built** (compila, mas nunca foi executado), **ran** (executado manualmente em um sistema em funcionamento), **e2e** (instalação e desinstalação completas em um sistema em funcionamento).

## Concluído

### Instalador

- Um único `plugin-launcher.exe` com todos os plugins incorporados (extraídos para Program Files, não para %TEMP%), CRT estático e janela escura em Win32/GDI. _ran_
- "Install WinRust" instala os componentes ausentes na ordem correta (topbar antes de rust-explorer) e atualiza quando todos já estão instalados; "Remove all" remove tudo na ordem inversa. _built_
- Uma instalação que falha desfaz suas próprias alterações (exceto durante a atualização de uma instalação funcional); os valores originais (WSearch, caixa de pesquisa, configurações do Explorer e serviços) são salvos em `HKLM\SOFTWARE\plugin-launcher\<plugin>` e restaurados durante a desinstalação; erros não fatais são registrados em `%ProgramData%\plugin-launcher\install.log`. _built_
- Ponto de restauração criado antes da instalação. _built_
- Um único botão instala o que estiver ausente **e** atualiza o que já estiver instalado, de modo que executar uma versão mais recente do `WinRust.exe` aplica suas correções a todos os plugins (v0.2.1). _built_
- Linha de comando: `list | install <plugin> --yes | update <plugin> | uninstall <plugin> | rollback --yes`, códigos de saída 0/1/2; a janela e a linha de comando utilizam as mesmas funções. Apenas os comandos somente de leitura foram executados. _ran (parcialmente)_
- `update` e `scripts\update-plugin.ps1`: reinstalam a partir de `payload\` no disco, sem precisar recompilar o instalador. _ran_
- Inglês como idioma padrão, alternância entre Русский/English no instalador e no menu da barra; a escolha é armazenada em `%ProgramData%\WinRust\lang.txt`. _tests_
- Correções após revisão (v0.3.0): rs-du mantém sua lista de quarentena ao lado do executável (somente administrador); `payload\` é localizado dois níveis acima apenas a partir de `target\release`; rtaskmgr e rust-explorer salvam e restauram as configurações do Gerenciador de Tarefas e das pastas de outros programas; rust-search restaura o Windows Search antes de remover seus arquivos; DiagTrack fica sob responsabilidade do debloat; um único analisador da saída de `reg query`, com teste. _tests, built_
- Organização do código: um único workspace do Cargo; instalador dividido em `install/` (um arquivo por plugin), `cli.rs` e `ui.rs`; crate compartilhado `winrust-common` (alternância de idioma e leitor MFT); uma única lista `payload.txt` para a compilação e os scripts; CI no GitHub Actions. _tests_

### Plugins

- **rust-search**: serviço de pesquisa NTFS + Alt+Space; desativa WSearch, bloqueia SearchHost.exe e oculta a caixa de pesquisa da barra de tarefas. Ao clicar com o botão direito em um resultado: renomear, enviar para a lixeira ou "excluir permanentemente + limpar o registro" (varredura → confirmação → exclusão; o registro é salvo previamente em `%LOCALAPPDATA%\rs-search\registry-backups\*.reg`). _tests, ran_
- **rtaskmgr-win32**: substitui o Gerenciador de Tarefas por meio de um IFEO Debugger (`taskmgr.exe` permanece intacto); em caso de panic, grava `%APPDATA%\rtaskmgr\crash.log` e abre o Taskmgr original. _tests, ran_
- **topbar**: barra inferior no estilo do Windows 11, no lugar da barra de tarefas; `topbar.exe --restore` restaura a barra de tarefas original. Instala junto com ela: atalhos do reshell, rs-du e o bloco do perfil do PowerShell (`src/install/profile.ps1`, Windows PowerShell 5.1 e pwsh 7). _ran; o bloco do perfil foi testado apenas em um perfil temporário_
- **reshell safety net**: se o topbar não estiver em execução enquanto a barra de tarefas estiver oculta, o reshell detecta isso duas vezes consecutivas (~10 s) e executa `topbar.exe --restore`. _tests; encerrar o topbar à força ainda não foi testado_
- **reshell hotkeys**: Win+D (mostrar área de trabalho) e Win+I (Configurações) funcionam sem o Explorer, assim como Win+E (v0.2.1). _ran_
- **Styles** (v0.3.0): uma engrenagem na extremidade esquerda da barra alterna entre Normal / Terminal / Floating dock / Tiling; a barra, o rust-explorer e a janela de pesquisa são atualizados imediatamente (por meio de uma mensagem de broadcast e de `%ProgramData%\WinRust\style.txt`). O dock não reserva espaço na tela e desliza para fora do caminho das janelas maximizadas; o Tiling posiciona a barra na parte superior e permite organizar as janelas lado a lado. _ran_
- **topbar look** (v0.3.0): o ícone de rede é desenhado (arcos de Wi-Fi conforme a intensidade do sinal, um X quando estiver offline e "!" quando não houver internet, seguindo o indicador de conectividade do Windows); novo painel de Wi-Fi (status, cartão da rede conectada com botão próprio de desconexão, redes salvas e abertas, linhas com animação de entrada); menus somente com texto e dicas de atalhos; seletor de idioma de entrada (clique esquerdo em ENG/RUS); o menu flutuante da bandeja aparece com efeito de fade e sua seta é animada. _ran_
- **rust-explorer**: gerenciador de arquivos (abas, ícones, operações de copiar/colar/excluir por IFileOperation, menu de contexto do shell, arrastar e soltar), desenha a área de trabalho e desativa o Explorer como shell por meio de `rs-shell --overlay` (no máximo 90 s após o login: o Windows 26300 deixou de gravar os marcadores de inicialização automática que antes eram aguardados, v0.2.1), além de reduzir os serviços ativos. _tests, ran_
- **rs-du**: limpador de disco (análise MFT, caches, `target`/`node_modules`, lixeira e quarentena de 30 dias), iniciado pelo menu da barra. _tests_
- **wiztree**: mapa em treemap do uso do disco, gerado a partir da MFT. _tests (apenas layout); não foi executado em um disco real_
- **photoview**: visualizador de fotos WIC, rotação EXIF e entrada em "Abrir com". _ran; orientações EXIF 5 e 7 não foram verificadas_
- **debloat**: serviços e políticas (telemetria, Copilot, Recall, anúncios e execução do Edge em segundo plano); valores anteriores armazenados em `%ProgramData%\winrust-debloat\undo.txt`, com suporte a `--undo`. _a simulação foi executada; `--apply`/`--undo` ainda não foram executados_

## Próximas etapas (em ordem)

1. **Testar em uma máquina virtual limpa com Windows 11**: instalação, desinstalação, rollback e reinicialização para cada plugin. Nada foi verificado em outra máquina até o momento.
2. **Um único registro de reversão para todos os plugins**: cada alteração (valor do registro, serviço ou arquivo) será registrada como "antes → depois" antes de ser aplicada; a desinstalação e o rollback reproduzirão o registro na ordem inversa. Isso substituirá a implementação manual de desinstalação de cada plugin. Requer a conclusão da etapa 1 e uma migração para instalações feitas pela versão atual.
3. **Manipulação do Registro por meio da API Win32** (`RegGetValueW`/`RegSetValueExW`), em vez de executar `reg.exe` e analisar sua saída.
4. **Releases**: SHA-256 junto ao executável, versão exibida na janela e verificação de atualizações.
5. **Assinatura de código** (remove o aviso do SmartScreen e reduz parte dos falsos positivos dos antivírus).
6. **Perfil do PowerShell**: instalação opcional de rg/fd/bat/eza/zoxide por meio do winget (uma caixa de seleção nas configurações do topbar).
7. **chrome-rs**: instalação real quando o navegador estiver pronto (consulte `plugins/chrome-rs/ROADMAP.md`).
8. **Win+S / botão Iniciar** abrem o rust-search em vez do Windows Search (alteração arriscada, que exige uma decisão separada).
9. **Unidades que não utilizam NTFS** no rust-search (atualmente ignoradas).

## Limitações conhecidas

- Os componentes do sistema são desativados ou contornados (de forma reversível), não removidos: excluir completamente o `taskmgr.exe` ou o Windows Search comprometeria a proteção do sistema.
- São necessários privilégios de administrador.
- Win+S e o botão Iniciar ainda abrem o Windows Search.
