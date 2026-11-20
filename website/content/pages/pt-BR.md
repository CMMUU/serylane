<!-- docs -->
# Primeiros passos e plataformas

Instale o pacote oficial para sua arquitetura e use sua assinatura ou configuração Mihomo. Veja os recursos atuais, limites por sistema e registros anteriores.

## Começar {#start}

Adicione e selecione sua assinatura ou importe um YAML em Perfis. Ligue o proxy do sistema e verifique uma conexão. Siga o [guia de uso](site:guide).

## Recursos atuais {#current-capabilities}

Gerencie assinaturas e nós e consulte conexões, registros e diagnósticos. Proxy de IA e seleção manual são alternativas. O [roteamento por domínio](site:routing) vem desativado; novas integrações com Codex e proxy por aplicativo permanecem desativados.

## TUN e plataformas {#tun-compatibility}

No macOS, use instalador completo assinado com Developer ID e autenticado pela Apple e autorize o auxiliar. No Windows, o TUN é experimental e exige privilégios de administrador. Não existe TUN no Linux; o proxy do sistema usa gsettings do GNOME.

## Download e arquitetura {#download-center}

Na [página de download](site:home#download) escolha x64 ou ARM64 conforme o sistema. Para Mac da série M use Chip da Apple. As versões estáveis são verificadas na [central de downloads](https://downloads.cmmuu.com/projects/serylane).

## Histórico {#history}

Registros anteriores ficam nos arquivos [em inglês](/docs/#history) e [em chinês simplificado](/zh/docs/#history). Eles não indicam qual arquivo está disponível hoje.

<!-- routing -->
# Definir uma saída por domínio

Envie as conexões correspondentes que já entram no núcleo para o grupo de políticas escolhido. O roteamento por domínio começa desativado.

## Ferramentas e domínios personalizados {#node-metadata}

Use predefinições para OpenAI, Claude e Gemini ou crie domínios próprios. `example.com` corresponde à raiz; `*.example.com` corresponde apenas aos subdomínios. Desmarcar uma ferramenta não remove seus domínios.

## Salvar e aplicar {#save-apply}

Escolha uma saída da assinatura ativa, salve o rascunho e confirme a ativação ou aplicação. Outras conexões mantêm as regras. Se o grupo desapareceu, escolha outro; não há troca silenciosa para conexão direta.

## Controles compartilhados {#global-controls}

O botão superior e a página de roteamento compartilham o estado. Atualizar o estado não inicia o núcleo nem alterna entre proxy do sistema e TUN.

## Confirmar funcionamento {#enablement}

Verifique o modo Regras, as regras carregadas e a entrada de tráfego. Salvar o rascunho não prova que está ativo. Confira a saída de uma conexão nova.

## Ao parar o proxy {#proxy-routing-state}

Parar o núcleo pausa o roteamento habilitado e mantém os ajustes. O app verifica novamente quando você inicia o proxy. O roteamento sozinho não inicia o proxy.

## Nós e estabilidade {#stability}

O proxy de IA usa de 1 a 10 nós elegíveis e não funciona junto com a seleção manual. Um único nó não tem reserva. Trocar de nó afeta apenas conexões futuras.

## Limites {#validation-boundaries}

Não descriptografa TLS, não separa caminhos cifrados nem altera o Codex. Novas integrações com o Codex, proxy por aplicativo e estratégia avançada antiga estão desativados. [Ferramentas de IA](site:guide#ai-tools).

<!-- verification -->
# Verificar downloads e atualizações

Confirme a publicação oficial, o nome exato, a arquitetura, o SHA-256 e a assinatura do projeto. Assinatura do sistema e permissões são verificações separadas.

## Origem {#download-origin}

Use o [site oficial](site:home#download) ou a [central de downloads](https://downloads.cmmuu.com/projects/serylane). Links para o código no GitHub não são outra fonte de instaladores.

## Comparar SHA-256 {#checksums}

Confira o nome exato e o valor em `SHA256SUMS.txt` da mesma versão. Não instale se não coincidirem. Use o nome do arquivo baixado no lugar dos exemplos.

- Windows: `Get-FileHash -LiteralPath ./installer.exe -Algorithm SHA256`
- macOS: `shasum -a 256 ./installer.dmg`
- Linux: `sha256sum ./installer.AppImage`

## Assinatura de atualização {#update-signatures}

O atualizador confere a assinatura do projeto. Para inspeção manual, use a assinatura, o manifesto e a chave pública da versão correspondente.

## Assinatura do sistema {#system-signing}

A assinatura do projeto não substitui o Authenticode do Windows, o Developer ID ou a autenticação do instalador pela Apple. Leia os registros da versão.

## TUN no macOS {#macos-tun}

Use o instalador completo assinado e autenticado pela Apple. Se o auxiliar antigo falhar com `EX_CONFIG`, é preciso remover o registro anterior e verificar o novo. Autorize em Itens de Início quando solicitado e confirme o tráfego.

## Download interrompido {#update-downloads}

O atualizador tenta GitHub direto, acelerador, GitHub pelo proxy Serylane ativo e central de downloads. Toda rota mantém versão, arquitetura, tamanho, hash e assinatura.

## Falha na instalação {#update-installation}

Confira o estágio da falha, espaço e permissões. Se o atualizador antigo continuar falhando, substitua o app pelo instalador completo correspondente e mantenha os ajustes.

## Verificações anteriores {#verification-history}

Consulte os registros [em inglês](/docs/verification#verification-history) ou [em chinês simplificado](/zh/docs/verification#verification-history).

<!-- privacy -->
# Privacidade e tratamento de dados

Site, central de downloads e aplicativo de desktop são sistemas separados. Acessar o site não inicia um proxy nem conecta o Codex.

## O que o site faz {#website-data}

Fornece conteúdo estático e informação pública sobre versões. Não usa contas, análise de acesso ou rastreadores de anúncios, não pede configuração e não acessa o proxy local.

## Preferência local {#theme-preference}

O navegador armazena apenas `serylane-theme` para aparência, sem enviar ao servidor. O idioma fica na URL, sem preferência salva ou cookies de rastreamento.

## Hospedagem {#hosting}

A hospedagem processa dados básicos como IP, horário, caminho e navegador para entrega e segurança.

## Downloads e links externos {#downloads}

O site confere dados públicos e o arquivo correspondente sem encaminhar cookies ou cabeçalhos de autorização do visitante. Destinos externos processam suas solicitações separadamente. [Verificação](site:verification).

## Aplicativo de desktop {#desktop-client}

Assinaturas e regras são funções do aplicativo. O roteamento vem desativado e não descriptografa TLS. Remova credenciais de URLs de assinatura antes de compartilhar.

## Relatar problemas {#reporting}

Use os [issues do GitHub](https://github.com/CMMUU/serylane/issues) sem publicar senhas, tokens, assinaturas ou configurações completas. Verifique um canal privado antes de divulgar detalhes de vulnerabilidades.
