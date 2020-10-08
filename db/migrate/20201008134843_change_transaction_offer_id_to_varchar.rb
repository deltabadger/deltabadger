class ChangeTransactionOfferIdToVarchar < ActiveRecord::Migration[5.2]
  def self.up
    change_table :transactions do |t|
      t.change :offer_id, :varchar
    end
  end

  def self.down
    uuid_regex = '[0-9a-fA-F]{8}\\-[0-9a-fA-F]{4}\\-[0-9a-fA-F]{4}\\-[0-9a-fA-F]{4}\\-[0-9a-fA-F]{12}'
    uuid_cast = "uuid USING CAST(
                  CASE WHEN
                    NOT offer_id ~ '#{uuid_regex}' THEN NULL
                    ELSE offer_id
                  END
                AS uuid)"
    change_table :transactions do |t|
      # This will cause all non-uuid offer_ids to be lost
      t.change :offer_id, uuid_cast
    end
  end
end
