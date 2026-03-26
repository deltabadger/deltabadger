class MakeCoingeckoOptional < ActiveRecord::Migration[8.1]
  def up
    # Mark all existing admin users as setup_completed
    # This ensures they skip the new simplified onboarding flow
    User.where(admin: true).update_all(setup_completed: true)

    # If setup_sync_status exists and is not completed, mark it as completed
    # This handles any users who were in the middle of the old onboarding flow
    sync_status_config = AppConfig.find_by(key: AppConfig::SETUP_SYNC_STATUS)
    if sync_status_config.present? && sync_status_config.value != AppConfig::SYNC_STATUS_COMPLETED
      sync_status_config.update!(value: AppConfig::SYNC_STATUS_COMPLETED)
    end
  end

  def down
    # No rollback needed - this is a one-way migration
  end
end
