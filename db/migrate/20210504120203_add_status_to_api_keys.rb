class AddStatusToApiKeys < ActiveRecord::Migration[5.2]
  def change
    add_column :api_keys, :status, :integer, null: false, default: 0

    reversible do |dir|
      dir.up do
        execute <<~SQL
          UPDATE api_keys
          SET 
            status = 1;
        SQL
      end
    end
  end
end
