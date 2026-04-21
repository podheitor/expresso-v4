# Mapeamento M365 → Expresso V4

> Baseado nas Service Descriptions oficiais da Microsoft (learn.microsoft.com/en-us/office365/servicedescriptions/)
> Data da pesquisa: 17 de abril de 2026
> Documento de referencia funcional alvo; nao representa implementacao completa no estado atual do repositorio.

---

## 1. Exchange Online → Expresso Mail

### 1.1 Clientes de Acesso

| Feature M365 | Expresso V4 | Protocolo/Padrão | Prioridade |
|-------------|-------------|-----------------|-----------|
| Outlook Web App (OWA) | Expresso Web (SvelteKit) | HTTP/HTTPS | P0 |
| Outlook para iOS/Android | PWA Mobile + Tauri futuro | IMAP4/SMTP | P0 |
| Outlook para Windows/Mac | IMAP client compat (Thunderbird, Apple Mail) | IMAP4rev2, EWS compat | P1 |
| Exchange ActiveSync (EAS) | EAS shim (compatibilidade móvel) | ActiveSync protocol | P2 |
| EWS (Exchange Web Services) | JMAP como substituto moderno | JMAP (RFC 8620) | P2 |
| SMTP relay externo | SMTP submission port 587 (STARTTLS) | RFC 5321, STARTTLS | P0 |

### 1.2 Caixa Postal

| Feature M365 | Expresso V4 | Detalhes Técnicos |
|-------------|-------------|------------------|
| Mailbox 50–100 GB por usuário | Quota configurável (padrão 50 GB) | MinIO + PostgreSQL metadata |
| Caixas compartilhadas (shared mailbox) | Shared mailbox via IMAP ACL | RFC 4314 (IMAP ACL) |
| Caixas de recursos (salas, equipamentos) | Resource mailboxes + CalDAV booking | RFC 4791 (CalDAV) |
| Grupos de distribuição (DL) | Listas de distribuição SMTP | RFC 2369 (List headers) |
| Microsoft 365 Groups | Grupos colaborativos (Mail+Calendar+Drive) | JMAP Groups |
| Mailbox de arquivo (In-Place Archive) | Expresso Archive — MinIO cold tier | IMAP NAMESPACE |
| Recuperação de item excluído (30 dias) | Soft delete com retenção configurável | |
| Recuperação de mailbox excluída (30 dias) | Backup automático + restore | |

### 1.3 Calendário

| Feature M365 | Expresso V4 | Protocolo |
|-------------|-------------|----------|
| Calendário pessoal | CalDAV server nativo | RFC 4791 |
| Calendários múltiplos por usuário | Multi-calendar por conta | CalDAV collections |
| Calendário de salas de reunião | Resource booking (accept/decline automático) | RFC 6638 (CalDAV Scheduling) |
| Calendário compartilhado | Compartilhamento via link ou permissão | CalDAV ACL |
| Eventos recorrentes | Recurrence rules completo (RFC 5545) | iCalendar RRULE |
| Fuso horário | VTIMEZONE em todos os eventos | iCalendar VTIMEZONE |
| Convites de reunião (iTIP) | RSVP via e-mail + UI | RFC 5546 (iTIP) |
| Free/Busy lookup | Disponibilidade em tempo real | RFC 6638 |
| Publishing calendário público | iCal feed público | |
| Integração e-mail ↔ calendário | Invite inline no Expresso Mail | |

### 1.4 Contatos

| Feature M365 | Expresso V4 | Protocolo |
|-------------|-------------|----------|
| Contatos pessoais | CardDAV server nativo | RFC 6352 |
| Catálogo de endereços global (GAL) | LDAP Directory + autocomplete | LDAP v3 (RFC 4511) |
| Contatos externos (external contacts) | Contatos compartilhados por tenant | |
| Grupos de contatos (contact groups) | Listas de contato por grupo | vCard GROUP |
| Foto de contato | Avatar em S3 | |
| Vinculação com redes sociais | Não planejado | |
| Universal Contact Card | Card unificado (e-mail + tel + cal) | vCard 4.0 (RFC 6350) |
| Sincronização móvel | CardDAV native + PWA | RFC 6352 |

### 1.5 Regras e Filtros

| Feature M365 | Expresso V4 | Padrão |
|-------------|-------------|-------|
| Inbox rules (regras do usuário) | Sieve rules por usuário | RFC 5228 (Sieve) |
| Transport rules (regras de fluxo de e-mail) | Transport layer rules (admin) | Sieve no servidor |
| Clutter (filtro de baixa prioridade) | Filtro por relevância (futuro AI) | |
| MailTips (avisos ao compor) | MailTips UI (OOO, tamanho, etc.) | |
| Auto-reply / Out-of-Office | OOO via Sieve (Vacation extension) | RFC 5230 (Sieve Vacation) |
| Regras de redirecionamento | Forward rules no Sieve | |
| Quarentena (usuário e admin) | Quarentena com URL de revisão | |
| Safe Senders / Blocked Senders | Whitelist/blacklist por usuário | |

### 1.6 Segurança de E-mail

| Feature M365 | Expresso V4 | Protocolo |
|-------------|-------------|----------|
| Anti-spam (múltiplos engines) | Rspamd (engine principal) + Bayesian | |
| Anti-malware | ClamAV + VirusTotal API opcional | |
| DKIM Signing (saída) | DKIM automático por domínio | RFC 6376 |
| DMARC enforcement | Verificação DMARC entrada + política saída | RFC 7489 |
| SPF validation | SPF check na entrada | RFC 7208 |
| S/MIME | S/MIME sign/encrypt, ICP-Brasil suporte | RFC 5751 |
| Microsoft Purview Message Encryption | Expresso Encrypted Mail (TLS + opcional E2EE) | |
| Advanced Message Encryption (E3/E5) | E2EE opcional por destinatário | |
| IRM (Information Rights Management) | Fase v1.2 | |
| Journaling (copia mensagens para arquivo) | Journal para compliance | |
| Safe Links (rewrite de URLs) | URL proxy scan (Phase futura) | |
| Safe Attachments (detonation) | Sandbox de anexos (futuro) | |

### 1.7 Arquivamento e Retenção

| Feature M365 | Expresso V4 | Normativa |
|-------------|-------------|----------|
| Exchange Online Archiving (E3/E5) | Expresso Archive — MinIO cold tier | |
| Retenção ilimitada (E3/E5) | Retenção configurável por categoria | LGPD art. 16 |
| Manual retention policies, labels, tags | Labels de retenção por usuário/admin | |
| Litigation Hold | Legal Hold — imutabilidade de caixa | LGPD art. 48, CADE |
| eDiscovery Content Search | Expresso eDiscovery (v1.1) | |
| In-Place Hold | Freeze de mailbox com auditoria | |
| PST Export | Export MBOX/EML (padrão aberto) | RFC 4155 (MBOX) |
| MRM (Messaging Records Management) | Políticas de ciclo de vida de e-mail | |

### 1.8 Alta Disponibilidade

| Feature M365 | Expresso V4 | SLA |
|-------------|-------------|-----|
| Replicação entre datacenters | Streaming replication PostgreSQL (2 réplicas síncronas) | |
| SLA 99.9% uptime | SLA 99.95% alvo | |
| Single item recovery | Soft delete 30 dias | |
| Deleted mailbox recovery | Backup diário + restore | RTO < 30min |

---

## 2. Microsoft Teams → Expresso Chat + Expresso Meet

### 2.1 Mensagens e Canais

| Feature M365 | Expresso V4 | Protocolo |
|-------------|-------------|----------|
| Canais Standard | Canais abertos por workspace | Matrix (client-server) |
| Canais Private | Canais privados com convite | Matrix (private rooms) |
| Canais Shared (entre orgs) | Federation com Matrix externo | Matrix federation |
| Chat 1:1 | Mensagens diretas | Matrix DMs |
| Chat em grupo | Grupos de mensagens | Matrix group rooms |
| Menções @usuario / @canal | @ mentions nativas | Matrix m.mention |
| Emojis, GIFs, stickers | Reaction e emoji native | Matrix reactions |
| Threads/replies | Thread replies | Matrix threads |
| Mensagens agendadas | Scheduled messages | |
| Modo urgente (ping repetido) | Priority notifications | |
| Loop components (co-edição inline) | Fase futura | |
| Edição/exclusão de mensagens | Edit/delete com histórico | Matrix redaction |
| Status de leitura | Read receipts | Matrix read receipts |
| Busca em mensagens | Full-text search chat | Tantivy |
| Arquivamento de canal | Archive channel | |

### 2.2 Reuniões e Webinars

| Feature M365 | Expresso V4 | Tecnologia |
|-------------|-------------|-----------|
| Reuniões agendadas (Outlook/Teams) | Expresso Meet (Fase 5) | WebRTC + SFU mediasoup |
| Reuniões ad-hoc | Meet now button | |
| Áudio/vídeo HD | WebRTC VP8/VP9/AV1 | WebRTC |
| Compartilhamento de tela | Screen share | getDisplayMedia API |
| Fundo virtual/desfoque | WebGL processing | WASM |
| Gravação de reunião | Gravação local + S3 | WebRTC Recording API |
| Transcrição automática | Whisper.cpp local (futuro) | |
| Breakout rooms | Fase futura | |
| Together Mode | Não planejado | |
| Tradução em tempo real | Fase futura | |
| Webinars (até 1.000 participantes) | Até 200 initial, HLS para escalar | HLS / WebRTC |
| Live Events (até 20.000) | HLS streaming | HLS |
| Whiteboard colaborativo | Excalidraw integration (futuro) | CRDT |
| Reações em reunião | Emoji reactions live | |
| Q&A em reunião | Q&A panel | |
| Salas de espera | Lobby with admit | |
| Controles de apresentador | Presenter controls | |
| Legenda ao vivo | Whisper (futuro) | |
| PSTN via Operator Connect | FreeSWITCH integration (futuro) | SIP |
| Direct Routing (PSTN) | SIP trunk (futuro) | RFC 3261 (SIP) |

### 2.3 Colaboração em Teams

| Feature M365 | Expresso V4 |
|-------------|-------------|
| Compartilhamento de arquivos no canal | Expresso Drive por workspace |
| Tabs (abas no canal) | Plugin tabs (WASM sandbox) |
| Apps no Teams (Power BI, Planner, etc.) | Expresso Extensions marketplace |
| Bots e conectores | Expresso Bot Builder (futuro) |
| Power Automate no Teams | Expresso Flows |
| Wiki por canal | Canal wiki page (markdown) |
| Planner integrado | Expresso Tasks (futuro) |
| Forms integrado | Expresso Forms (futuro) |

---

## 3. SharePoint Online + OneDrive → Expresso Drive

### 3.1 Armazenamento e Arquivos

| Feature M365 | Expresso V4 | Protocolo |
|-------------|-------------|----------|
| OneDrive 1 TB por usuário | Quota configurável (padrão 50 GB, expansível) | S3-compat (MinIO) |
| SharePoint Team Sites | Workspaces colaborativos | WebDAV |
| Bibliotecas de documentos | Pastas tipadas com versionamento | WebDAV Collections |
| Listas (SharePoint Lists) | Expresso Tables (Fase futura) | |
| Sync client Windows/Mac | Tauri desktop sync (futuro) + WebDAV | WebDAV |
| Sync para iOS/Android | PWA file access + mobile app futuro | DAV |
| Upload de pasta | Folder upload | |
| Drag and drop | DnD nativo | |
| Arquivos grandes (>2 GB) | tus resumable upload protocol | tus.io |

### 3.2 Compartilhamento

| Feature M365 | Expresso V4 | Segurança |
|-------------|-------------|----------|
| Link "qualquer pessoa" (anonymous) | Link anônimo com TTL | Token assinado JWT |
| Link "organização" | Link interno por tenant | RBAC |
| Link com permissão (leitura/edição) | Link com permission level | |
| Link com expiração | TTL configurável | |
| Link com senha | Password-protected link | bcrypt hash |
| Compartilhamento externo (convidado) | Guest user com conta temporária | |
| Aceitar/declinar convites | Expiry invitation workflow | |

### 3.3 Versionamento e Recuperação

| Feature M365 | Expresso V4 |
|-------------|-------------|
| Versões automáticas (até 500) | Versões ilimitadas (MinIO + snapshot) |
| Restauração de versão | UI de histórico de versões |
| Lixeira de site (1 estágio) | Soft delete 30 dias |
| Recycle bin de coleção de sites (2 estágio) | Admin restore 60 dias |
| Proteção ransomware (Microsoft 365 alert) | Detecção de upload em massa anômalo (futuro) |

### 3.4 Metadados e Search

| Feature M365 | Expresso V4 | Tecnologia |
|-------------|-------------|-----------|
| Colunas de metadados customizados | File metadata extensible | PostgreSQL JSONB |
| Content Types | File type classification | |
| Managed metadata (Term Store) | Taxonomy service (futuro) | |
| Busca full-text em documentos | Tantivy + Apache Tika extraction | |
| Busca por metadados | PostgreSQL full-text + índice | |
| People search | Directory search | LDAP + OpenSearch |
| Microsoft Search integration | Expresso Search | |
| Filtros de busca (refiners) | Faceted search | |

### 3.5 Compliance e Auditoria (Drive)

| Feature M365 | Expresso V4 | Normativa |
|-------------|-------------|----------|
| Audit log de arquivos | Audit log append-only | LGPD art. 37 |
| DLP em arquivos (E3/E5) | DLP scan inline (Fase v1.2) | LGPD art. 46 |
| Sensitivity Labels em arquivos | Labels de classificação (Fase v1.2) | |
| Information Rights Management | Fase futura | |
| eDiscovery em SharePoint | Expresso eDiscovery abrange Drive | |
| Retention policies em arquivos | Políticas LGPD nativas | |

---

## 4. Office Apps Online → LibreOffice Online Upstream

### 4.1 Aplicativos

| M365 | LibreOffice Online Equiv | Formatos Principais |
|------|--------------------------|-------------------|
| Word Online | Writer Online | .odt, .docx, .rtf, .txt, .fodt |
| Excel Online | Calc Online | .ods, .xlsx, .csv, .fods |
| PowerPoint Online | Impress Online | .odp, .pptx, .fodp |
| OneNote Online | Expresso Notes (futuro — Markdown) | .md, .html |
| Visio Online | Draw Online (LO Draw) | .odg, .vsd, .svg |
| Project Online | Expresso Project (futuro) | .mpx |
| Access Online | Não planejado (obsoleto) | |
| Publisher Online | Não planejado (obsoleto) | |
| Forms | Expresso Forms (futuro) | |
| Sway | Não planejado | |
| Clipchamp | Não planejado | |

### 4.2 Funcionalidades de Co-edição (LOOL)

| Feature M365 | LibreOffice Online | Protocolo |
|-------------|-------------------|----------|
| Co-edição em tempo real | Sim (WebSocket) | WOPI + WebSocket |
| Presença de usuário no documento | Sim (cursor colorido) | |
| Comentários inline | Sim | |
| Track changes | Sim | |
| Revisão de documento | Sim | |
| Histórico de versões inline | Via WOPI Bridge (Expresso Drive) | WOPI |
| Modo de leitura / apresentação | Sim | |
| Macro support | Basic Macros (com restrições de segurança) | |
| Extensões/Add-ins | LO Extensions (sandboxed) | |

---

## 5. Microsoft Entra ID → Expresso Identity

| Feature M365 | Expresso V4 | Protocolo |
|-------------|-------------|----------|
| SSO (Single Sign-On) | Keycloak OIDC/SAML2 | OIDC 1.0, SAML 2.0 |
| MFA — TOTP (Authenticator) | TOTP nativo (RFC 6238) | RFC 6238 |
| MFA — WebAuthn/FIDO2 | WebAuthn nativo | FIDO2/WebAuthn W3C |
| MFA — SMS (fallback) | SMS via gateway | TOTP fallback |
| **gov.br OIDC (bronze/prata/ouro)** | **🇧🇷 NATIVO — diferencial único** | OpenID Connect + ACR |
| ICP-Brasil cert auth | Keycloak x.509 certificator | PKI, ICP-Brasil |
| Passwordless | WebAuthn passkey | FIDO2 |
| Conditional Access | OPA (Open Policy Agent) engine | Rego policies |
| Privileged Identity Management | RBAC + time-limited roles | |
| B2B Collaboration (guest users) | Federation + guest accounts | OIDC federation |
| SSPR (self-service password reset) | SSPR com TOTP verification | |
| SCIM 2.0 provisioning | SCIM 2.0 server nativo | RFC 7643/7644 |
| Active Directory federation | LDAP federation via Keycloak | LDAP v3 |
| Azure AD Connect (hybrid) | Keycloak LDAP User Federation | |
| Entitlement Management (access packages) | Role + Group management | |
| Access Reviews | Revisão de acesso periódica | |
| Continuous Access Evaluation | Token refresh com policy check | |

---

## 6. Microsoft Purview / Compliance → Expresso Compliance

| Feature M365 | Expresso V4 | Normativa | Fase |
|-------------|-------------|----------|------|
| Audit Standard | Audit log append-only (PostgreSQL + WORM S3) | GSI IN 01/2020 | v1.0 |
| Audit Premium (180 dias) | Audit log retenção 1 ano (LGPD mínimo) | LGPD | v1.0 |
| eDiscovery Standard | Busca imutável + exportação MBOX | LGPD art. 48 | v1.1 |
| eDiscovery Premium | Legal Hold + chain of custody | | v2.0 |
| Content Search | Search across mail + drive + chat | | v1.1 |
| DLP — E-mail | DLP scan no SMTP pipeline | LGPD art. 46 | v1.2 |
| DLP — SharePoint/OneDrive | DLP scan em uploads | | v1.2 |
| DLP — Teams | DLP em mensagens de chat | | v2.0 |
| Sensitivity Labels | Labels de classificação de dados | | v1.2 |
| Information Protection | Proteção por label (criptografia, watermark) | | v2.0 |
| Insider Risk Management | Detecção de comportamento anômalo | | v3.0 |
| Communication Compliance | Revisão de comunicações | | v3.0 |
| Information Barriers | Separação de grupos (Chinese wall) | | v3.0 |
| Customer Key (BYOK) | HSM/Vault on-prem (NATIVO) | GSI soberania | v1.0 |
| Double Key Encryption | E2EE client-side (opcional) | | v1.1 |
| Compliance Manager | Dashboard de conformidade LGPD/GSI | | v1.2 |
| Priva — Privacy Management | Gestão de consentimento + DSAR | LGPD arts. 7-22 | v1.0 |
| Priva — Subject Rights Requests | Portal de direitos do titular | LGPD art. 18 | v1.1 |
| Records Management | Lifecycle de registros gov | e-ARQ Brasil | v2.0 |
| Data Connectors | Conectores de dados externos | | v3.0 |

---

## 7. Power Platform → Expresso Flows + Forms + BI

| Feature M365 | Expresso V4 | Fase |
|-------------|-------------|------|
| Power Automate | Expresso Flows (webhooks + connectors) | v2.0 |
| Power Apps | Expresso Apps (WASM low-code) | v3.0 |
| Power BI | Integração Grafana + Metabase (OSS) | v2.0 |
| Power Pages | Site intranet builder (futuro) | v3.0 |
| Power Virtual Agents | Expresso Bot Builder | v3.0 |
| AI Builder | Expresso AI local | v4.0 |

---

## 8. Microsoft 365 Admin Center → Expresso Admin

| Feature M365 | Expresso V4 | Prioridade |
|-------------|-------------|-----------|
| User management | SCIM 2.0 + UI admin | P0 |
| Group management | Grupos com RBAC | P0 |
| Billing & subscriptions | Multi-tenant billing (futuro) | P2 |
| Domain management | DNS + certificado por domínio | P1 |
| Service health dashboard | Status page + alertas | P1 |
| Message center | Changelog interno de lançamentos | P2 |
| Reports (usage analytics) | Grafana dashboards | P1 |
| Microsoft 365 Apps deployment | N/A (LOOL + cliente web) | |
| Intune (MDM/MAM) | MDM básico para PWA (futuro) | v3.0 |
| Exchange admin center | Admin de mailboxes, regras, quarentena | P1 |
| Teams admin center | Admin de workspaces, políticas | P2 |
| SharePoint admin center | Admin de quotas, permissões, sites | P2 |
| Security & Compliance admin | Expresso Security Dashboard | P1 |
| RBAC para admins | Roles: SuperAdmin, TenantAdmin, HelpDesk | P0 |

---

## 9. Microsoft 365 Copilot → Expresso AI (Fase Futura)

| Feature M365 Copilot | Expresso AI Plan |
|---------------------|-----------------|
| Copilot in Outlook | LLM local on-prem (Llama 3.x / Qwen 2.5) via Ollama |
| Copilot in Teams (resumo de reunião) | Whisper.cpp + LLM summarizer |
| Copilot in Word/Excel | LLM + LOOL macro integration |
| Copilot in PowerPoint | LLM + Impress |
| Microsoft Graph grounding | Expresso Graph API (RAG sobre dados do tenant) |
| BizChat (enterprise chat with data) | Expresso Assistant UI |
| Semantic search | Embeddings locais + Qdrant vector DB |
| Image generation | SDXL local (opcional, hardware permitting) |
| **Diferencial absoluto** | Dados NUNCA saem do servidor — 100% on-prem |

---

## 10. Outros Serviços M365

| Serviço M365 | Expresso V4 | Prioridade |
|-------------|-------------|-----------|
| Microsoft Bookings | Expresso Booking (futuro) | v3.0 |
| Microsoft Forms | Expresso Forms | v2.0 |
| Microsoft Planner + To-Do | Expresso Tasks | v2.0 |
| Microsoft Stream (vídeo interno) | MinIO HLS streaming | v3.0 |
| Viva Insights (bem-estar digital) | Não planejado | - |
| Viva Connections (intranet) | Expresso Portal | v3.0 |
| Viva Learning | Não planejado | - |
| Universal Print | Não planejado | - |
| Windows 365 (Cloud PC) | Não planejado | - |
| Microsoft Loop (collaborative pages) | Expresso Pages (futuro) | v3.0 |
| Microsoft Whiteboard | Excalidraw integration | v4.0 |
| Microsoft Clipchamp | Não planejado | - |
| Microsoft Sway | Não planejado | - |
| Microsoft Designer (AI design) | Não planejado | - |

---

## 11. Gaps Onde Expresso V4 Supera M365

| Dimensão | M365 Limitação | Expresso V4 Vantagem |
|----------|---------------|---------------------|
| **Soberania de dados** | Dados na nuvem Microsoft (EUA por padrão) | 100% on-premise nacional, data residency BR garantida |
| **gov.br OIDC** | Sem integração nativa | Nativo — bronze/prata/ouro, CPF/CNPJ |
| **Custo** | R$ 100–400/usuário/mês M365 E3/E5 | Custo de infraestrutura apenas |
| **Privacidade** | Telemetria extensiva para Microsoft | Zero telemetria — configurável |
| **E2EE** | Limitado (E5+) | E2EE opcional para todos os planos |
| **ICP-Brasil** | Suporte limitado/indireto | ICP-Brasil nativo na camada de assinatura |
| **Auditoria** | Audit log de 180 dias (padrão) | 1 ano mínimo, imutável, LGPD-compliant |
| **Customização** | Limitada por política MS | Fork e customização ilimitada |
| **AI on-prem** | Copilot processa dados na cloud MS | LLM local, dados nunca saem do servidor |
| **Formatos** | Lock-in proprietário (.msg, .eml Microsoft) | ODF, MBOX, vCard, iCal — padrões abertos |
| **Interoperabilidade** | Ecossistema fechado | IMAP, CalDAV, CardDAV, JMAP, Matrix |

---

## 12. Editais Gov BR — Requisitos Comuns (Baseado em Termos de Referência Conhecidos)

Órgãos que já contrataram M365: SERPRO, STF, STJ, TST, Câmara dos Deputados, Ministério da Fazenda, Banco Central, TRF (1ª a 6ª regiões)

### Requisitos Típicos Exigidos em Editais

```
SEÇÃO TÉCNICA — REQUISITOS MÍNIMOS PARA SUÍTE COLABORATIVA

1. COMUNICAÇÃO E COLABORAÇÃO
   1.1 Serviço de e-mail corporativo — capacidade mínima 50 GB/caixa postal
   1.2 Calendário compartilhado com gestão de salas e recursos
   1.3 Plataforma de mensagens instantâneas e colaboração em equipe
   1.4 Videoconferência integrada com gravação e transcrição
   1.5 Compartilhamento e co-edição de documentos em tempo real

2. ARMAZENAMENTO
   2.1 Armazenamento pessoal mínimo de 1 TB por usuário
   2.2 Armazenamento colaborativo por equipe/departamento
   2.3 Sincronização com dispositivos móveis e desktops
   2.4 Versionamento de arquivos com histórico mínimo de 30 versões

3. APLICATIVOS DE ESCRITÓRIO
   3.1 Editor de texto com suporte a .docx e ODF
   3.2 Planilha eletrônica com suporte a .xlsx e ODS
   3.3 Editor de apresentações com suporte a .pptx e ODP
   3.4 Acesso via browser sem instalação local obrigatória

4. SEGURANÇA E CONFORMIDADE
   4.1 Autenticação multifator (MFA) obrigatória
   4.2 SSO integrado ao serviço de identidade do órgão
   4.3 Criptografia de dados em trânsito (TLS 1.2+) e em repouso
   4.4 Controle de acesso baseado em funções (RBAC)
   4.5 Proteção contra spam e malware em e-mail
   4.6 Auditoria de acessos e atividades com retenção mínima de 90 dias
   4.7 Conformidade com LGPD (Lei 13.709/2018)
   4.8 Data residency em território brasileiro
   4.9 Backup com RPO ≤ 24 horas e RTO ≤ 8 horas
   4.10 SLA de disponibilidade mínimo de 99,9%

5. ADMINISTRAÇÃO
   5.1 Painel centralizado de gerenciamento (admin center)
   5.2 Integração com Active Directory / LDAP do órgão
   5.3 Provisionamento automatizado de usuários (SCIM ou AD sync)
   5.4 Gerenciamento remoto de dispositivos móveis (MDM)
   5.5 Relatórios de uso e atividade

6. REQUISITOS GOVERNAMENTAIS ESPECÍFICOS
   6.1 Suporte a autenticação via gov.br (OpenID Connect)
   6.2 Compatibilidade com ICP-Brasil para assinaturas digitais
   6.3 Conformidade com e-PING (Padrões de Interoperabilidade)
   6.4 Certificação de segurança reconhecida pelo DSIC/GSI
   6.5 Dados processados e armazenados em território nacional
```

---

*Última atualização: 17 de abril de 2026*
*Fontes: Microsoft Learn Service Descriptions, conhecimento técnico consolidado, editais publicados*
