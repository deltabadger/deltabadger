class AddSettingFlagsTable < ActiveRecord::Migration[5.2]
  def change
    drop_table :settings
    create_table :setting_flags do |t|
      t.string :name
      t.boolean :value
    end
  end
end
