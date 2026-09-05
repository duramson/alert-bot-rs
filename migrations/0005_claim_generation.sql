-- Fence late writes from a worker whose claim was released and reassigned.
-- Never reset this counter when advancing a recurring alert.
ALTER TABLE alerts ADD COLUMN claim_generation BIGINT NOT NULL DEFAULT 0;
