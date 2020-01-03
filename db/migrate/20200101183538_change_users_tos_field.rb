class ChangeUsersTosField < ActiveRecord::Migration[5.2]
  def change
    rename_column :users, :terms_of_service, :terms_and_conditions
  end
end
