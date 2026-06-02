# Expresso v4 — Guia de Demonstração

Sobe a suíte inteira (backend + webmail) num comando, com dados de exemplo e um
usuário de login pronto. **Pré-requisito: Docker + Docker Compose v2** na máquina.

## Subir

```bash
# na máquina que vai hospedar a demo (a que tem Docker):
bash scripts/demo-up.sh
```

O script detecta o IP da LAN automaticamente. Para forçar um IP específico
(ex.: o IP fixo do servidor de demo):

```bash
HOST_IP=192.168.15.125 bash scripts/demo-up.sh
```

> O `HOST_IP` é o IP **da máquina que roda o Docker** — é o endereço que outras
> máquinas da rede e o Keycloak vão usar. Não é o IP do seu notebook cliente.

A primeira execução **compila as imagens em Rust** (pode levar vários minutos).
Execuções seguintes sobem em segundos.

## Acessar (de qualquer máquina na rede)

| | URL | Credenciais |
|---|---|---|
| **Webmail** | `http://<HOST_IP>:3000` | **patricia / patricia** |
| Painel Admin | `http://<HOST_IP>:8101` | (login via Keycloak) |
| Keycloak | `http://<HOST_IP>:8080` | admin / `demo-admin-pass` |
| Grafana | `http://<HOST_IP>:3001` | admin / `demo-grafana` |

A usuária **patricia** já vem com agenda (2 eventos), 5 contatos, um arquivo no
Drive e uma pasta de e-mail — o webmail abre com conteúdo, não vazio.

## Operar

```bash
docker compose ps                  # status dos serviços
docker compose logs -f expresso-web   # logs do webmail
docker compose down                # derruba (mantém os dados)
docker compose down -v             # derruba E apaga os dados (reset total)
```

## Se algo não abrir

1. `docker compose ps` — algum serviço fora de `healthy`? Veja os logs dele.
2. A 1ª subida ainda está **compilando**? Aguarde; `expresso-web` é o último a ficar pronto.
3. `ERR_CONNECTION_REFUSED` em `:3000` = o `expresso-web` ainda não subiu (ou o build falhou) — `docker compose logs expresso-web`.
4. Login não funciona = o `seed-realm.sh` não rodou. Reexecute:
   `KC_ADMIN_PASS=demo-admin-pass KC_URL=http://<HOST_IP>:8080 bash deploy/keycloak/seed-realm.sh`

> **Segurança:** os segredos do `.env` gerado são fracos de propósito (demo).
> Não use este fluxo em produção — veja `docs/DEPLOYMENT.md` para deploy real.
