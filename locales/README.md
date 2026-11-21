# Catálogo de tradução em preparação

`pt-BR.json` reúne as traduções já existentes no fork para revisão e migração.
O programa ainda usa as traduções compiladas no código; este arquivo **ainda não é um pacote de idioma instalável**.

Para atualizar o levantamento, execute `python scripts/i18n-catalog.py export`.
Para comparar com outro checkout, execute `python scripts/i18n-catalog.py audit --source CAMINHO`.
A ferramenta preserva entradas já revisadas, indica textos sem tradução e confere os campos de formatação. Ela usa extração por padrões e pode precisar de revisão para expressões Rust que não sejam textos literais.
Acrescente `--strict` para que textos faltantes ou ambíguos resultem em erro em verificações automatizadas.

As entradas em `needs_review` têm mais de uma tradução para o mesmo texto em inglês. É necessário atribuir contexto antes de escolher uma tradução. Traduções importadas automaticamente também precisam de revisão humana.

Uma implementação instalável precisa incluir carregamento seguro de catálogos externos, seleção de pacotes no instalador, idioma padrão do Windows, fallback em inglês, validação de versões e campos de formatação, e testes de atualização offline. O instalador poderá ser distribuído em uma pasta ou ZIP com os catálogos separados do executável, sem exigir serviço em nuvem.
