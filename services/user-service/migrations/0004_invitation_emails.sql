-- When the invitation's link was last emailed (NULL: never, e.g. no mail server). A resend
-- replaces the token, so the previous link stops working.
ALTER TABLE invitations ADD COLUMN last_sent_at timestamptz;
