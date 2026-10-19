# Argos 📅🏍⛽

Assistente pessoal no Telegram, em Rust: agenda (Google), tarefas que cobram até você fazer, memória, manutenção do veículo e abastecimento por foto. Feito pra rodar numa VM pequena: fica em **~4–6 MB** de RAM parado e passa de **~8 MB** de pico lendo duas fotos. A inteligência vem da API da Anthropic (Claude), com a sua própria chave.

[English summary](README.en.md)

O que ele faz:

- **Agenda:** conversa com a Google Agenda: *"marca dentista quinta 15h"*, *"o que tenho amanhã?"*, *"apaga o date de sexta"* (apagar sempre pede botão).
- **Tarefas:** lista própria do bot. Cobra até você marcar feito, dentro de uma janela de horário. Cada cobrança sem resposta fica mais espaçada (3 h, 4h30, 6h45…) e cai nos horários em que você costuma aparecer, aprendidos por dia da semana (`/rotina`). Uma vez por semana, o que ficou parado vem numa lista pra você dizer quando dá pra fazer: dia útil em horário comercial, dia útil quando estiver livre, começo do mês ou amanhã. Durante um evento da agenda ele segura a cobrança e avisa quando o evento acaba. Cada tarefa pode ter intervalo, janelas, condição e pausa próprios. `/tarefas` lista.
- **Hábitos:** tarefa criada "como hábito" (*ração do gato a cada 2 dias desde a última vez, como hábito*). Cada vez que você faz, ele grava em `dados/tracker.csv`.
- **Memória:** guarda fatos que você conta (*"a Carla é minha chefe"*), com validade opcional (ou presa ao fim de um evento da agenda), e pede revisão a cada 14 dias. `/memoria` mostra tudo com botões.
- **Desfazer:** toda gravação vem com botão **↩️ desfazer**: abastecimento, manutenção, tarefa feita, evento e memória. Também dá pra pedir *"desfaz isso"*.
- **Abastecimento (opcional):** foto da bomba + odômetro (+ cupom fiscal) → o Claude lê → rascunho com botões → grava no CSV do veículo e responde com o consumo e o custo por km.
- **Bom-dia:** às 6h, uma mensagem só e sem som com o que você tem pro dia: agenda, chuva, tarefas cobrando e o resto do dia, manutenção vencida e a revisão da memória, cada um com seus botões.
- **Manutenção (opcional):** confere os prazos de cada item (km, meses ou dias) e avisa o que venceu no bom-dia, com o botão "✅ fiz". Item que você não marca volta cada vez mais espaçado, até `manutencao_max_dias` (3).
- **Chuva (opcional):** às 21h avisa em que horários de amanhã a chance de chuva passa do limite, e o bom-dia confirma o resto do dia. Também responde *"vai chover amanhã?"*. Usa a Open-Meteo, sem chave e sem passar pelo Claude.
- **Alerta de temporal (opcional):** avisos do INMET pro município (só vermelho dispara) e a previsão hora a hora num ponto (rajada, chuva forte, granizo). Responde no `/temporal`.
- **Onde abastecer (opcional):** `/postos` ordena os postos da cidade por **custo por 100 km**: o preço da semana na pesquisa da ANP ÷ quanto aquele posto rendeu no seu histórico.
- **Duas pessoas:** cada uma com o seu bot. Um pode consultar a agenda, a memória e as tarefas do outro (menos o que for privado) e mandar recado.
- **Atalhos sem IA:** agenda do dia, tarefas, manutenção, custos, chuva e desfazer são respondidos pelo código, sem gastar a API (`/atalhos`).

## Instalar

Precisa de um bot do Telegram ([@BotFather](https://t.me/BotFather), `/newbot` → token) e de uma chave da API da Anthropic ([console.anthropic.com](https://console.anthropic.com/)). Google Agenda e veículo são opcionais.

```bash
cargo build --release                       # → target/release/argos
mkdir ~/argos-ana && cd ~/argos-ana
ARGOS_DIR=$PWD argos --init                 # nome, pronome, nome do bot, token do BotFather, chave da Anthropic
ARGOS_DIR=$PWD argos                        # sobe; o log mostra um código de 6 dígitos
```

1. **Parear:** mande o código de 6 dígitos pro bot, no privado. A partir daí ele só responde pra esse chat (grava `credenciais/chat_id`).
2. **Veículo (opcional):** `argos --veiculo moto "Honda CG 160" 2024` (ou `carro`). Isso cria `dados/motor/` com intervalos de manutenção genéricos; confira com o manual em `dados/motor/veiculos/*.json`. Sem veículo, a foto, a manutenção e o `/custos` ficam desligados.
3. **Google Agenda (opcional):** cada pessoa usa o **próprio** projeto no Google:
   - no [Google Cloud Console](https://console.cloud.google.com/), crie um projeto e ligue a **Google Calendar API**;
   - crie um *OAuth client* do tipo **Desktop app** e baixe o JSON como `credenciais/google_client.json`;
   - na tela de consentimento, ponha o app **em produção**: em "Testing" o refresh token vence em 7 dias;
   - numa máquina com navegador, rode `argos --google-login` e copie o `google_token.json` pro `credenciais/` do servidor.

   Sem Google, *"marca X"* vira tarefa e a agenda some do prompt.
4. **Extras (opcionais):** chuva, alerta de temporal, postos da ANP, fuso e um parágrafo seu no prompt (`prompt_extra`) ligam por chave no `configs.json`. Todas as chaves e os padrões estão em [`exemplos/configs.exemplo.json`](exemplos/configs.exemplo.json).
5. **Serviço:** [`exemplos/argos.service`](exemplos/argos.service). Uma cópia por pessoa, cada uma com o seu `ARGOS_DIR` e o seu token.

`argos --checar-config` valida o `configs.json` e lista o que está ligado. Rode antes de reiniciar depois de mexer nele.

## Configuração

Onde ele procura `configs.json` e `dados/`: em `$ARGOS_DIR`; senão, na pasta do binário (se tiver `configs.json` lá); senão, no diretório atual. Caminho relativo no `configs.json` (`motor_dir`, `cred_dir`) vale a partir dessa pasta.

Só `usuario` é obrigatória. O resto tem padrão:

| chave | o quê |
|---|---|
| `usuario` | `{"nome", "ela", "sobre", "cidade", "assistente"}`: quem usa (vai pro prompt). `sobre`, `cidade` e `assistente` (padrão "o Argos") são opcionais |
| `fuso` | padrão `America/Sao_Paulo`. Os fusos do Brasil e Lisboa vão no binário; pra outro, acrescente em `.cargo/config.toml` e recompile |
| `cobrar_a_cada_h`, `janela_cobranca` | cobrança padrão das tarefas (cada tarefa pode ter as suas) |
| `cobranca_fator`, `cobranca_max_h` | cada cobrança sem resposta multiplica o intervalo (padrão 1,5, até 72 h) |
| `horarios_cobranca`, `antecedencia_min` | horários fixos de cobrança (`["07:30", "21:30"]`), somados aos aprendidos da sua rotina; a cobrança sai `antecedencia_min` (20) antes do costume |
| `hora_bom_dia` | hora do bom-dia (padrão `"06:00"`); tarefa sem hora cai no 1º horário de cobrança daquele dia da semana |
| `manutencao_repetir_dias`, `manutencao_max_dias` | item vencido sem resposta volta depois disso × `cobranca_fator`, até o máximo (padrão 3 e 3) |
| `horario_comercial`, `dia_discussao` | janelas da condição "dia útil, horário comercial"; dia da lista das tarefas paradas (padrão `"sexta"`, `""` desliga) |
| `clima_lat`, `clima_lon`, `chuva_limiar` | liga o aviso de chuva; `chuva_aviso: false` deixa só a consulta |
| `alerta_geocode` (código IBGE), `alerta_nome`, `alerta_lat`, `alerta_lon` | liga o alerta de temporal |
| `postos_municipio`, `postos_estado` | liga o `/postos` (pesquisa semanal da ANP, baixada por `argos --anp` num processo à parte) |
| `agenda_parceiro` | `{"nome", "calendar_id", "ela", "dir"}`: a agenda de outra pessoa, só consultada. Ela compartilha com a conta Google deste bot (Configurações → Compartilhar com pessoas específicas → "Ver todos os detalhes do evento"). `dir` (o `ARGOS_DIR` do bot dela, na mesma máquina) liga a consulta da memória e das tarefas dela e o envio de recado |
| `manutencao_veiculos` | ex. `["onix"]`: aviso de manutenção só desses |
| `postos_json` | `postos.json` de outra instância (em comum); com ele, esta não baixa a ANP |
| `prompt_extra` | um parágrafo seu no meio das regras do prompt (aceita `{ele}`, `{dele}`, `{do_nome}`) |
| `google_escopos_extra` | escopos a mais no `--google-login`, pra um programa seu usar o mesmo token |

Credenciais ficam em `cred_dir` (padrão `<ARGOS_DIR>/credenciais`, criada com permissão 0700 pelo `--init`): `telegram_token`, `chat_id`, `anthropic_key` e, com Google, `google_client.json` e `google_token.json`.

**Nunca deixe duas instâncias rodando com o mesmo token**: dois `getUpdates` brigam (409 Conflict) e as mensagens se perdem entre eles.

## Rodar

```bash
./argos.sh                         # bot (compila o release se precisar)
./argos.sh --ler bomba.jpg odo.jpg # só testa a leitura das fotos, não grava (usa a API)
./argos.sh --agenda                # próximos 7 dias
./argos.sh --temporal              # o que o /temporal responderia agora
./argos.sh --selftest              # monta tudo numa pasta temporária e mostra a memória (sem rede)
./argos_test.sh                    # testes (só fixtures; nenhum chama a API paga)
```

## Biblioteca

O crate também é uma biblioteca (`src/lib.rs`). Um programa seu pode usar o Telegram, o token do Google, a config e os utilitários sem copiar código: `argos = { path = "../argos" }`. É assim que dá pra acoplar uma coleta própria rodando por um timer do systemd, sem mexer no bot.

## Glibc antiga (ex.: Oracle Linux 9)

Binário compilado num sistema novo pode pedir uma glibc que o servidor não tem (`pidfd_spawnp@GLIBC_2.39`). Nesse caso, compile num container da mesma distro: `./build-ol9.sh` gera `target/ol9/release/argos`, que pede no máximo GLIBC_2.34. Compilar no próprio servidor também funciona, mas LTO com 500 MB de RAM é lento. Com SELinux, binário copiado por `scp` chega com `user_home_t`: rode `chcon -t bin_t argos` antes de subir o serviço.

## Dependências (e por quê)

| crate | por quê |
|---|---|
| `ureq` 2 (só feature `tls`) | HTTP bloqueante e pequeno, sem tokio. TLS com **rustls** + `webpki-roots` embutido: não depende da libssl nem do bundle de certificados do sistema. Um `Agent` só pra tudo (keep-alive). |
| `serde` + `serde_json` (`preserve_order`) | JSON. `preserve_order` deixa schemas, `estado.json` e resultados de ferramenta na ordem de inserção. |
| `chrono` + `chrono-tz` (`filter-by-regex`) | datas e fuso. O `.cargo/config.toml` filtra o tzdb pros fusos do Brasil e Lisboa. |
| `base64` | as fotos vão em base64 pro Claude. |
| `unicode-normalization` | busca sem acento (NFD sem as marcas). |
| `miniz_oxide` (só `with-alloc`) | inflate pra abrir o xlsx semanal da ANP; o zip e o XML são lidos à mão. Só roda no `argos --anp`, nunca no processo do bot. |
| `regex` (só `std` + `unicode-perl`) | correções em texto livre (`km 3.810, litros 8,52`) e atalhos. |

Não existe SDK oficial da Anthropic em Rust: o bot faz `POST /v1/messages` na mão, com novas tentativas em 429 (respeitando o `retry-after`), em 5xx/529 e em falha de conexão.

## Memória medida (`--selftest`)

| momento | VmRSS |
|---|---|
| processo subiu | ~4 MB |
| bot montado (TLS, regex, estado, credenciais) | ~4,5 MB |
| depois de montar o corpo de 2 fotos de 400 KB | pico (VmHWM) ~8,6 MB |
| parado depois de tudo | ~5 MB |

## Custo

~US$ 0,01–0,02 por abastecimento (Sonnet) e ~US$ 0,01 por mensagem de agenda (Haiku). Os atalhos não gastam nada.

## Licença e contribuições

[FSL-1.1-ALv2](LICENSE.md) (Functional Source License). Dá pra usar, modificar e hospedar pra você e pros seus; só não dá pra oferecer como serviço concorrente. Cada versão vira Apache-2.0 dois anos depois de publicada.

Bug ou ideia: abra uma issue. Pull request só com cessão de direitos combinada antes.
