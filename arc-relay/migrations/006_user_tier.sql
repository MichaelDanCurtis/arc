-- 006_user_tier.sql
--
-- Add the free/pro tier column to users for the entitlement gating
-- described in REQUIREMENTS.md §1 "Tiers" row.
--
-- Existing users default to 'free'. Upgrade paths to 'pro' (forthcoming):
--   - Beacon-bundled provisioning (server marks Beacon-licensed users on
--     first auth — implementation lands with §1 Tiers wiring).
--   - Standalone Stripe purchase (post-billing milestone).
--
-- The CHECK constraint keeps the set closed: only ('free', 'pro') values
-- are valid. If a future tier is needed (e.g. 'enterprise'), the constraint
-- gets a new migration, not a code-side coercion.

ALTER TABLE users ADD COLUMN tier TEXT NOT NULL DEFAULT 'free';
ALTER TABLE users ADD CONSTRAINT users_tier_check CHECK (tier IN ('free', 'pro'));
