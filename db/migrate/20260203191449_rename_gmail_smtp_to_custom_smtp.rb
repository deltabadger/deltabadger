class RenameGmailSmtpToCustomSmtp < ActiveRecord::Migration[8.0]
  def up
    # Rename key: smtp_gmail_email → smtp_username
    execute <<-SQL
      UPDATE app_configs SET key = 'smtp_username'
      WHERE key = 'smtp_gmail_email'
    SQL

    # Rename key: smtp_gmail_password → smtp_password
    execute <<-SQL
      UPDATE app_configs SET key = 'smtp_password'
      WHERE key = 'smtp_gmail_password'
    SQL

    # Update provider value from 'gmail_smtp' to 'custom_smtp'
    # Values are encrypted with attr_encrypted, so we need Ruby to handle this
    provider_record = AppConfig.find_by(key: 'smtp_provider')
    if provider_record && provider_record.value == 'gmail_smtp'
      provider_record.value = 'custom_smtp'
      provider_record.save!

      # Insert default host/port for migrated Gmail configs
      AppConfig.set('smtp_host', 'smtp.gmail.com')
      AppConfig.set('smtp_port', '587')
    end
  end

  def down
    # Rename key: smtp_username → smtp_gmail_email
    execute <<-SQL
      UPDATE app_configs SET key = 'smtp_gmail_email'
      WHERE key = 'smtp_username'
    SQL

    # Rename key: smtp_password → smtp_gmail_password
    execute <<-SQL
      UPDATE app_configs SET key = 'smtp_gmail_password'
      WHERE key = 'smtp_password'
    SQL

    # Revert provider value from 'custom_smtp' to 'gmail_smtp'
    provider_record = AppConfig.find_by(key: 'smtp_provider')
    if provider_record && provider_record.value == 'custom_smtp'
      provider_record.value = 'gmail_smtp'
      provider_record.save!
    end

    # Remove host/port records
    AppConfig.find_by(key: 'smtp_host')&.destroy
    AppConfig.find_by(key: 'smtp_port')&.destroy
  end
end
