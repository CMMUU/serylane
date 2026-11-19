# Guia de uso do Serylane

Instale o Serylane, importe sua assinatura ou um YAML local, escolha um modo de proxy e aprenda a configurar o roteamento, atualizar e resolver problemas.

## Baixar e instalar
<!-- anchor: install -->

Obtenha a versão estável verificada no [site oficial](https://serylane.cmmuu.com/pt-br/#download) ou na [central de downloads](https://downloads.cmmuu.com/projects/serylane). Nós, assinaturas e contas de IA não estão incluídos.

- Windows: x64 para PCs Intel ou AMD de 64 bits; ARM64 para Windows em Arm.
- macOS: use Chip da Apple para a série M ou Intel para um Mac Intel. Abra o DMG, arraste o app para Aplicativos e use macOS 13 ou posterior.
- Linux: escolha o AppImage da sua arquitetura, torne-o executável e abra-o.

## Primeiros passos
<!-- anchor: quick-start -->

1. Em Assinaturas, informe nome e URL da assinatura Mihomo ou Clash Meta, valide e adicione. Para YAML local, abra em Perfis, confira e ative.
2. Confirme que o perfil desejado está selecionado. Selecionar uma configuração não inicia o proxy.
3. Ligue o proxy do sistema na barra superior, abra um site conhecido e verifique em Conexões se a solicitação chegou ao núcleo e seguiu a regra esperada.

## Escolher um modo de proxy
<!-- anchor: proxy-modes -->

- **Proxy do sistema**: para aplicativos que leem a configuração do sistema. No Linux usa o gsettings do GNOME.
- **TUN**: para aplicativos que ignoram o proxy do sistema. No macOS use o instalador completo assinado e autenticado pela Apple; autorize o auxiliar em Itens de Início e Extensões se solicitado. No Windows é experimental e exige privilégios de administrador. Não está implementado no Linux.
- **Porta local**: configure `127.0.0.1` e a porta mista no aplicativo, via HTTP ou SOCKS5. Não use a porta de controle.

O auxiliar pronto não comprova que o tráfego entrou no túnel. Confira o modo e uma conexão real.

## Gerenciar assinaturas
<!-- anchor: subscriptions -->

Atualize um cartão ou todas as assinaturas. Uso e configuração são atualizados separadamente; falhas mantêm a última configuração utilizável. Edite em Mais ou com o botão direito e atualize após trocar a URL. Em caso de erro, confira se o endereço é de uma assinatura Mihomo ou Clash Meta e verifique o caminho de download escolhido.

## Seleção manual ou proxy de IA
<!-- anchor: nodes -->

Os modos são mutuamente exclusivos. Selecione um nó de um grupo e meça a latência, ou use o proxy de IA com 1 a 10 nós qualificados. Um único nó não tem reserva. Aplique o nó e aguarde a confirmação ou restaure a seleção automática. A troca só afeta novas conexões.

## Ferramentas de IA, domínios e Codex
<!-- anchor: ai-tools -->

O roteamento por domínio vem desligado. Só afeta conexões que já entram no núcleo no modo Regras. Selecione ferramentas ou domínios e uma saída, salve o rascunho e então ative ou aplique. `example.com` é o domínio raiz; `*.example.com` corresponde apenas aos subdomínios. Consulte [Roteamento por domínio](https://serylane.cmmuu.com/pt-br/docs/routing).

No macOS ou Windows, uma CLI Codex compatível aceita `codex --enable respect_system_proxy` para ler o proxy do sistema apenas naquela execução. O comando não ativa PAC nem adiciona troca automática de rota. Veja a [documentação da OpenAI](https://learn.chatgpt.com/docs/enterprise/managed-configuration#use-the-system-proxy-for-native-client-requests) e confirme com uma solicitação real concluída.

## Verificar e instalar atualizações
<!-- anchor: updates -->

Em Configurações → Atualização, confira a versão e a origem efetiva. O aplicativo tenta GitHub direto, o acelerador do projeto, GitHub pelo proxy Serylane ativo e a central de downloads. Versão, arquitetura e assinatura são verificadas. Se um atualizador antigo falhar, substitua o aplicativo com o instalador oficial correspondente e preserve a configuração. [Verificação](https://serylane.cmmuu.com/pt-br/docs/verification).

## Solucionar problemas
<!-- anchor: troubleshooting -->

- Aplicativo não passa pelo proxy: confira se o proxy do sistema ou TUN está ativo e consulte Conexões.
- TUN no macOS aguarda autorização: verifique os itens de início. Pare o proxy antes de reassociar o auxiliar.
- Porta ocupada: abra os detalhes técnicos e espere o TUN liberar a porta antes de tentar de novo.
- HTTP 401 indica que o endpoint pode ser alcançado, não que a conta ou a resposta do modelo foi validada.

No relato inclua versão, sistema, arquitetura, modo, horário e erro sem dados confidenciais. Não envie URLs de assinatura nem chaves.
