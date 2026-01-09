class AddSetupCompletedToUsers < ActiveRecord::Migration[8.1]
  def up
    add_column :users, :setup_completed, :boolean, default: false, null: false

    # Set existing admins as completed if API key exists
    if AppConfig.coingecko_api_key.present?
      User.where(admin: true).update_all(setup_completed: true)
    end
  end

  def down
    remove_column :users, :setup_completed
  end
end
