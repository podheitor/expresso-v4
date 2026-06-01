-- Revert: drop the per-tenant LDAP federation configuration.

DROP TABLE IF EXISTS ldap_config;
