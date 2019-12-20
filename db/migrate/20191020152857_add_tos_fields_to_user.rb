class AddTosFieldsToUser < ActiveRecord::Migration[5.2]
  def change
    add_column :users, :terms_and_conditions, :boolean
    add_column :users, :updates_agreement, :boolean
  end
end
