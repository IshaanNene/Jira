-- A clone is linked to its original: "PAY-12 clones PAY-3".
ALTER TABLE issue_links DROP CONSTRAINT issue_links_link_type_check;
ALTER TABLE issue_links ADD CONSTRAINT issue_links_link_type_check
    CHECK (link_type IN ('blocks', 'relates', 'duplicates', 'depends_on', 'clones'));
