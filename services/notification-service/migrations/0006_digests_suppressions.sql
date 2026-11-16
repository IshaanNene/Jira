-- Activity digests (one email per hour or day instead of one per notification) and the
-- list of addresses not to email because they bounced or complained.

ALTER TABLE notification_preferences
    ADD COLUMN email_digest text NOT NULL DEFAULT 'off' CHECK (email_digest IN ('off', 'hourly', 'daily'));

-- A digest row waits until its recipient's window closes (every pending digest row of a
-- person shares one `send_after`) and is sent together with the others as one email.
ALTER TABLE email_deliveries ADD COLUMN digest boolean NOT NULL DEFAULT false;
ALTER TABLE email_deliveries DROP CONSTRAINT email_deliveries_status_check;
ALTER TABLE email_deliveries ADD CONSTRAINT email_deliveries_status_check
    CHECK (status IN ('pending', 'sending', 'sent', 'skipped', 'suppressed', 'failed'));
CREATE INDEX email_deliveries_digest_idx ON email_deliveries (tenant_id, recipient_id) WHERE digest AND status = 'pending';

-- A mailbox is the same in every workspace, so this list is global (read and written on the
-- global database only) and not row-level secured; no API lists it. Addresses are kept as
-- SHA-256 of the trimmed, lower-cased address, never in the clear.
CREATE TABLE email_suppressions (
    address_hash bytea PRIMARY KEY,
    -- bounce: the receiving server said the mailbox does not exist; complaint: marked as spam.
    reason       text NOT NULL CHECK (reason IN ('bounce', 'complaint')),
    created_at   timestamptz NOT NULL DEFAULT now()
);
