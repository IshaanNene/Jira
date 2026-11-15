-- A workspace can require two-step verification of everyone who signs in to it. identity-service
-- reads this with each membership check and refuses a workspace token to anyone without it.
ALTER TABLE organizations ADD COLUMN require_two_step boolean NOT NULL DEFAULT false;
