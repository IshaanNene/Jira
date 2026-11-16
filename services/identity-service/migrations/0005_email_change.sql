-- Changing an account's address: the link goes to the new address and is bound to the current
-- one (as every link is), so it dies if the address changes some other way first.
ALTER TABLE email_tokens ADD COLUMN new_email citext;
ALTER TABLE email_tokens DROP CONSTRAINT email_tokens_purpose_check;
ALTER TABLE email_tokens ADD CONSTRAINT email_tokens_purpose_check
    CHECK (purpose IN ('verify_email', 'reset_password', 'change_email'));
ALTER TABLE email_tokens ADD CONSTRAINT email_tokens_new_email_check
    CHECK ((purpose = 'change_email') = (new_email IS NOT NULL));
