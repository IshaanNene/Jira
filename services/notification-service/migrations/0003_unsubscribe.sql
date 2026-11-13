-- One-click unsubscribe (RFC 8058): the URL for the List-Unsubscribe header of each email.
ALTER TABLE email_deliveries ADD COLUMN unsubscribe_url text;
