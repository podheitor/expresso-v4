# Billing — Runbook

Modelo de cobrança do Expresso V4: **planos com preço fixo por tenant + faturas
internas**, sem gateway de pagamento externo. O pagamento é registrado
manualmente por um administrador.

Tudo vive no serviço `expresso-admin`.

## Modelo

- Cada tenant tem um plano em `tenants.plan` (`standard` | `professional` |
  `enterprise`).
- `billing_plans` é o catálogo global de preços (um preço mensal por plano, em
  centavos, moeda `BRL` por padrão).
- `billing_invoices` guarda uma fatura por `(tenant, período)` — o período é o
  primeiro dia do mês coberto. Status: `pending` | `paid` | `void`.
- A geração é **idempotente** por `(tenant, período)` via constraint única →
  re-rodar um mês nunca duplica, só preenche o que faltava.

## Telas

| Rota | Quem | O quê |
| --- | --- | --- |
| `/billing.html` | super-admin | Catálogo de planos (editar preço), gerar/marcar faturas por tenant, **gerar o mês para todos os tenants**. |
| `/my-billing.html` | tenant-admin | Plano + preço + histórico de faturas da própria organização (read-only). |
| `/my-billing/invoices/:id` | tenant-admin | Fatura imprimível (print → PDF pelo navegador), escopada ao próprio tenant. |
| `/my-billing/invoices.csv` | tenant-admin | Exporta o histórico de faturas da própria organização em CSV. |

As telas `/my-billing*` são acessíveis a qualquer admin e resolvem o tenant a
partir da sessão (`/auth/me`), nunca de um id na URL.

## Add-ons por uso (excedente)

Além do preço-base fixo, cada plano tem uma **franquia** e um **preço de
excedente** por dimensão:

| Dimensão | Franquia | Excedente |
| --- | --- | --- |
| Usuários (seats) | `included_seats` | `seat_overage_cents` por usuário acima |
| Armazenamento | `included_storage_gb` | `storage_overage_cents_per_gb` por GB acima |

O armazenamento medido = soma de bytes de mailbox (`messages`) + arquivos vivos
do Drive (`drive_files` não-deletados); GB são arredondados **para cima**. Seats
= contagem de `users` do tenant.

Na geração da fatura, o uso atual é medido e, quando excede a franquia **e** o
preço de excedente é > 0, a fatura ganha linhas `seat_overage` /
`storage_overage` além da linha `base`. O total da fatura
(`billing_invoices.amount_cents`) é a soma das linhas; o detalhamento aparece na
fatura imprimível. Deixe o excedente em 0 para um plano puramente fixo (default).

Re-gerar um período **re-mede** o uso (idempotente por `(tenant, período)`).

## Definir preços

Via tela (`/billing.html` → colunas de preço-base, franquias e excedentes na
linha do plano → "Salvar") ou via API:

```bash
curl -X PUT https://admin.example/api/v1/admin/billing/plans/professional \
  -H 'content-type: application/json' \
  -b "$ADMIN_COOKIE" \
  -d '{"monthly_price_cents": 9900}'   # super-admin
```

## Gerar faturas

### Manual, um tenant

`/billing.html` → seção "Faturas por tenant" → selecione o tenant → "Gerar
fatura do mês".

### Manual, todos os tenants

`/billing.html` → seção "Gerar faturas do mês (todos os tenants)" → escolha o
mês → "Gerar para todos". Roda um único `INSERT … SELECT` sobre
`tenants JOIN billing_plans`.

### Agendado (cron mensal)

Endpoint de máquina para um scheduler externo (k8s CronJob, systemd timer):

```
POST /api/v1/admin/billing/run?period=YYYY-MM
Header: X-Billing-Token: <segredo>
```

Configure o segredo na variável de ambiente **`BILLING__RUN_TOKEN`** do
`expresso-admin`. O endpoint é **fail-closed**: se a variável não estiver
definida, responde `503` e não faz nada (ao contrário das rotas `/internal/*`
de confiança-LAN, porque este endpoint muta dados de cobrança e pode estar
exposto). O token é comparado em tempo constante.

Resposta: `{"period":"YYYY-MM-01","generated":<n>}` onde `n` é o número de
faturas efetivamente criadas (já existentes não contam — idempotente).

Exemplo de CronJob (k8s), no dia 1 de cada mês às 03:00:

```yaml
apiVersion: batch/v1
kind: CronJob
metadata:
  name: billing-monthly-run
spec:
  schedule: "0 3 1 * *"
  jobTemplate:
    spec:
      template:
        spec:
          restartPolicy: OnFailure
          containers:
            - name: run
              image: curlimages/curl:8
              env:
                - name: BILLING__RUN_TOKEN
                  valueFrom: { secretKeyRef: { name: billing, key: run-token } }
              command:
                - sh
                - -c
                - >
                  curl -fsS -X POST
                  "http://expresso-admin:8101/api/v1/admin/billing/run?period=$(date +%Y-%m)"
                  -H "X-Billing-Token: $BILLING__RUN_TOKEN"
```

> Idempotência torna o retry seguro: se o Job falhar e re-rodar, faturas já
> criadas no mês são ignoradas.

## Marcar uma fatura como paga / anulada

Via tela (`/billing.html`, botões por linha) ou API:

```bash
curl -X PATCH https://admin.example/api/v1/admin/invoices/<invoice_id> \
  -H 'content-type: application/json' \
  -b "$ADMIN_COOKIE" \
  -d '{"status": "paid"}'   # paid | void | pending — super-admin
```

`paid` carimba `paid_at`; qualquer outro status o limpa.

## Segurança

- Definir preço, gerar e marcar faturas exigem `super_admin`.
- Ver/imprimir/exportar faturas é escopado ao tenant do principal
  (`require_tenant_match` / filtro `tenant_id` explícito) — um tenant-admin não
  vê fatura de outro tenant mesmo adivinhando o id (404).
- A conexão do admin roda com `app.tenant_id` NULL (RLS permite), então as
  queries de fatura carregam um `WHERE tenant_id = $1` explícito como
  defesa-em-profundidade.
- O endpoint de cron é fail-closed e usa comparação de token em tempo constante.
