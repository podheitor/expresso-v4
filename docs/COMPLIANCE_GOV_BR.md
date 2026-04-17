# Conformidade Gov BR — Expresso V4

## 1. LGPD (Lei 13.709/2018)

| Artigo | Requisito | Implementação Expresso V4 | Status |
|--------|-----------|--------------------------|--------|
| Art. 7 | Base legal para tratamento | Consentimento explícito + legítimo interesse | v1.0 |
| Art. 9 | Transparência ao titular | Privacy notice, cookie consent, data mapping | v1.0 |
| Art. 15 | Término do tratamento | Data retention policies automáticas | v1.0 |
| Art. 16 | Conservação necessária | Retenção mínima legal por categoria | v1.0 |
| Art. 18 | Direitos do titular | Portal DSAR self-service | v1.1 |
| Art. 37 | Registro de operações | Audit log imutável | v1.0 |
| Art. 46 | Segurança técnica | TLS 1.3, AES-256, RBAC, MFA | v1.0 |
| Art. 47 | Sigilo profissional | Separation of duties, acesso mínimo | v1.0 |
| Art. 48 | Notificação de incidente | Incident response + notificação ANPD | v1.1 |

## 2. e-PING (Padrões de Interoperabilidade)

| Dimensão | Padrão Exigido | Expresso V4 | Status |
|----------|---------------|-------------|--------|
| Intercâmbio de dados estruturados | XML, JSON, CSV | JSON (primário), CSV (export) | ✅ |
| E-mail | SMTP/IMAP, S/MIME | SMTP/IMAP4rev2, S/MIME + ICP-Brasil | ✅ |
| Calendário | iCal, CalDAV | iCalendar RFC 5545, CalDAV RFC 4791 | ✅ |
| Contatos | vCard, CardDAV | vCard 4.0 RFC 6350, CardDAV RFC 6352 | ✅ |
| Autenticação | SAML 2.0, OpenID Connect | OIDC 1.0 + SAML 2.0 via Keycloak | ✅ |
| Documentos | ODF (ISO 26300) | ODF nativamente via LibreOffice | ✅ |
| Segurança | TLS 1.2+, ICP-Brasil | TLS 1.3, ICP-Brasil nativo | ✅ |
| Acessibilidade | WCAG 2.1 AA | WCAG 2.1 AA obrigatório no UI | 🔄 |

## 3. GSI — Segurança da Informação

| Instrução | Requisito Principal | Implementação |
|-----------|--------------------|--------------| 
| IN 01/2020 | Política de segurança da informação | PSI.md documentada, OPA policies |
| IN 05/2021 | Gestão de vulnerabilidades | CVE scanning (cargo-audit, npm audit), patching |
| NC 14/2012 | Gestão de incidentes | IRP + notificação CERT.br automática |
| NC 17/2013 | Gestão de continuidade | BCP, DRP, simulações anuais |

## 4. ICP-Brasil

| Aplicação | Implementação | Padrão |
|-----------|--------------|-------|
| Assinatura de e-mail (S/MIME) | Certificado ICP-Brasil A1/A3 | MP 2.200-2/2001 |
| Assinatura de documentos | LTV signatures via LibreOffice | DOC-ICP-15 |
| Autenticação de usuário | x.509 client cert no Keycloak | ICP-Brasil hierarquia |
| Carimbo de tempo | TSA integration (SERPRO ou AC-Tempo) | RFC 3161 |

## 5. gov.br OIDC

```rust
// Configuração do provider gov.br
pub const GOV_BR_ISSUER: &str = "https://sso.acesso.gov.br";
pub const GOV_BR_AUTH_URL: &str = "https://sso.acesso.gov.br/authorize";
pub const GOV_BR_TOKEN_URL: &str = "https://sso.acesso.gov.br/token";
pub const GOV_BR_USERINFO_URL: &str = "https://sso.acesso.gov.br/userinfo";
pub const GOV_BR_JWKS_URL: &str = "https://sso.acesso.gov.br/jwk";

// Níveis de confiança (Authentication Context Reference)
pub enum GovBrTrustLevel {
    Bronze,  // acr: "https://acesso.gov.br/assurance/loa-1" - senha gov.br
    Prata,   // acr: "https://acesso.gov.br/assurance/loa-2" - validação biométrica
    Ouro,    // acr: "https://acesso.gov.br/assurance/loa-3" - certificado ICP-Brasil
}

// Claims disponíveis do gov.br
pub struct GovBrClaims {
    pub sub: String,         // CPF hash
    pub cpf: String,         // CPF formatado
    pub name: String,        // Nome completo
    pub email: Option<String>,
    pub phone_number: Option<String>,
    pub cnpj: Option<String>,
    pub acr: String,         // nível de confiança
    pub picture: Option<String>,
    pub profile: Option<String>,
    pub email_verified: bool,
}

// Acesso mínimo exigido para operações
pub enum RequiredTrustLevel {
    AnyAuthenticated,  // Leitura de e-mail pessoal
    Bronze,            // Operações padrão
    Prata,             // Operações sensíveis (DLP, audit)
    Ouro,              // Operações críticas (admin, legal hold)
}
```

## 6. Armazenamento de Dados — Requisitos Governamentais

| Tipo de Dado | Retenção Mínima | Normativa |
|-------------|----------------|----------|
| Logs de acesso a sistemas | 6 meses | GSI IN 01/2020 |
| Registros de auditoria | 1 ano | GSI |
| E-mails corporativos | 5 anos | e-ARQ Brasil (documentos) |
| Documentos administrativos | Conforme tabela de temporalidade | CONARQ |
| Dados pessoais (LGPD) | Apenas enquanto necessário | LGPD art. 15 |
| Logs de incidentes de segurança | 2 anos | CERT.br recomendação |

## 7. Certificação e Homologação

| Certificação | Responsável | Prazo |
|-------------|-------------|-------|
| Análise de conformidade LGPD | DPO interno + jurídico | Antes do deploy produção |
| Pentest (OWASP Top 10) | Red team externo | A cada major release |
| Auditoria de código (SAST) | cargo-audit + semgrep | CI/CD contínuo |
| Avaliação GSI/DSIC | DSIC (se órgão federal) | Antes de homologar |
| Certificação ICP-Brasil da aplicação | AC credenciada | Fase 1 (e-mail) |
