# Deploy Cloud — Expresso V4

Infraestrutura para ambiente de piloto em 4 VMs na nuvem.

## Topologia

```
Internet
    │ HTTPS 443 / IMAP 993 / SMTP 587
    ▼
┌─────────────────────────────────────┐
│  VM: app  (8 vCPU / 16 GB / 60 GB) │
│  Nginx (TLS termination)            │
│  13 serviços Rust + Keycloak        │
└──────────────┬──────────────────────┘
               │ rede privada
    ┌──────────┼──────────┬──────────────┐
    ▼          ▼          ▼              ▼
┌────────┐ ┌────────┐ ┌────────┐  ┌──────────┐
│  data  │ │  meet  │ │  obs   │  │ (futuro) │
│8vC/32G │ │8vC/ 8G │ │2vC/ 8G │
│Postgres│ │WebRTC  │ │Prom    │
│MinIO   │ │SFU     │ │Grafana │
│NATS    │ │        │ │Alertmgr│
│Redis   │ │        │ │        │
└────────┘ └────────┘ └────────┘
```

## Pré-requisitos locais

```bash
pip install ansible
ansible-galaxy collection install community.docker
```

## Passos

### 1. Provisionar VMs na OMID

Usar os cloud-init de cada VM ao criar:

| VM | Arquivo | vCPU | RAM | Disco |
|----|---------|:----:|:---:|-------|
| app | `cloud-init-app.yaml` | 8 | 16 GB | 60 GB SSD |
| data | `cloud-init-data.yaml` | 8 | 32 GB | 60 GB OS + 500 GB NVMe + 2 TB SSD |
| meet | `cloud-init-meet.yaml` | 8 | 8 GB | 40 GB SSD |
| obs | `cloud-init-obs.yaml` | 2 | 8 GB | 200 GB SSD |

Substituir `<SUA_CHAVE_SSH_PUBLICA>` pela chave pública antes de subir.

### 2. Preencher inventory e env

```bash
# Copiar e editar o inventory
cp deploy/cloud/ansible/inventory.ini.example deploy/cloud/ansible/inventory.ini
# Preencher todos os <PLACEHOLDER> com IPs reais

# Copiar e editar os arquivos de ambiente
cp deploy/cloud/env/app.env.example  deploy/cloud/env/app.env
cp deploy/cloud/env/data.env.example deploy/cloud/env/data.env
cp deploy/cloud/env/meet.env.example deploy/cloud/env/meet.env
cp deploy/cloud/env/obs.env.example  deploy/cloud/env/obs.env
# Preencher senhas e IPs em cada arquivo
```

### 3. Validar pré-requisitos

```bash
bash deploy/cloud/bootstrap.sh --check
```

### 4. Deploy completo

```bash
bash deploy/cloud/bootstrap.sh
```

Ou passo a passo:

```bash
bash deploy/cloud/bootstrap.sh --step data
bash deploy/cloud/bootstrap.sh --step app
bash deploy/cloud/bootstrap.sh --step meet
bash deploy/cloud/bootstrap.sh --step obs
bash deploy/cloud/bootstrap.sh --step smoke
```

### 5. DNS (após obter os IPs públicos)

| Registro | Tipo | Valor |
|----------|------|-------|
| `<DOMINIO>` | A | IP público VM app |
| `auth.<DOMINIO>` | A | IP público VM app |
| `mail.<DOMINIO>` | A | IP público VM app |
| `meet.<DOMINIO>` | A | IP público VM meet |
| `admin.<DOMINIO>` | A | IP público VM app |
| `grafana.<DOMINIO>` | A | IP público VM obs |
| `<DOMINIO>` | MX | `mail.<DOMINIO>` |
| `<DOMINIO>` | TXT | SPF: `v=spf1 a mx ~all` |
| `default._domainkey.<DOMINIO>` | TXT | chave DKIM (gerar via `scripts/dkim-keygen.sh`) |

### 6. Firewall externo (security group OMID)

| VM | Porta | Protocolo | Origem |
|----|-------|-----------|--------|
| app | 22 | TCP | IP ops |
| app | 80, 443 | TCP | 0.0.0.0/0 |
| app | 143, 587, 993 | TCP | 0.0.0.0/0 |
| meet | 22 | TCP | IP ops |
| meet | 443 | TCP | 0.0.0.0/0 |
| meet | 10000–20000 | **UDP** | 0.0.0.0/0 |
| data | 22 | TCP | IP ops |
| data | 5432, 6379, 9000, 4222 | TCP | rede privada |
| obs | 22 | TCP | IP ops |
| obs | 3001 | TCP | 0.0.0.0/0 (ou IP ops) |

## Estrutura de arquivos

```
deploy/cloud/
├── README.md               ← este arquivo
├── bootstrap.sh            ← orquestrador de deploy
├── cloud-init-app.yaml     ← cloud-init VM app
├── cloud-init-data.yaml    ← cloud-init VM data
├── cloud-init-meet.yaml    ← cloud-init VM meet
├── cloud-init-obs.yaml     ← cloud-init VM obs
├── compose-app.yaml        ← serviços Rust + Keycloak
├── compose-data.yaml       ← Postgres + MinIO + NATS + Redis
├── compose-meet.yaml       ← expresso-meet
├── compose-obs.yaml        ← Prometheus + Grafana + Alertmanager
├── ansible/
│   ├── inventory.ini       ← IPs das VMs (preencher)
│   └── playbook.yml        ← playbook completo
├── nginx/
│   └── nginx-cloud.conf.j2 ← nginx com TLS + todos os vhosts
├── prometheus/
│   └── prometheus-cloud.yml.j2 ← scrape das 4 VMs
└── env/
    ├── app.env.example
    ├── data.env.example
    ├── meet.env.example
    └── obs.env.example
```
