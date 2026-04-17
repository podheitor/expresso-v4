-- Initialize Expresso V4 databases
CREATE DATABASE keycloak;
GRANT ALL PRIVILEGES ON DATABASE keycloak TO expresso;

-- Enable extensions on main DB
\c expresso
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
CREATE EXTENSION IF NOT EXISTS "pg_trgm";
CREATE EXTENSION IF NOT EXISTS "btree_gin";
