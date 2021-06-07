class AddTwoFactorAuthorizationToUsers < ActiveRecord::Migration[5.2]
  def change
    add_column :users, :otp_secret_key, :string
    add_column :users, :otp_module, :integer, default: 0

    Rake::Task['update_users_with_otp_secret_key'].invoke
  end
end
