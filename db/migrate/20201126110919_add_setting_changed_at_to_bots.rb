class AddSettingChangedAtToBots < ActiveRecord::Migration[5.2]
  def change
    add_column :bots, :settings_changed_at, :datetime

    reversible do |dir|
      dir.up do
        execute <<~SQL
          UPDATE bots
          SET 
            settings_changed_at = updated_at;
        SQL
      end
    end
  end
end
