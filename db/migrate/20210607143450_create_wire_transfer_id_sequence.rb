class CreateWireTransferIdSequence < ActiveRecord::Migration[5.2]
  def up
    execute <<-SQL
      CREATE SEQUENCE wire_transfer_id_seq 
      INCREMENT 1 
      MINVALUE 1 
      MAXVALUE 9223372036854775807 
      START 900 
      CACHE 1;
    SQL
  end

  def down
    DROP SEQUENCE wire_transfer_id_seq;
  end
end
