-- Sprint #570: vacation auto-deactivation por data.
-- Adiciona `deactivate_at TIMESTAMPTZ` ao `user_vacation`.
-- NULL = nunca auto-desativa. Quando preenchido, o worker horário
-- seta `enabled=false` e limpa `sieve_script` quando deactivate_at <= now().

ALTER TABLE user_vacation
    ADD COLUMN IF NOT EXISTS deactivate_at TIMESTAMPTZ;
