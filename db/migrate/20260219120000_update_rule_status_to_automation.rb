class UpdateRuleStatusToAutomation < ActiveRecord::Migration[8.1]
  def up
    add_column :rules, :settings_changed_at, :datetime

    # Remap status values from old enum (inactive=0, active=1)
    # to Automation::Statusable enum (created=0, scheduled=1, stopped=2)
    # active(1) → scheduled(1): no change needed
    # inactive(0) → stopped(2): must remap
    execute "UPDATE rules SET status = 2 WHERE status = 0"
  end

  def down
    # Remap back: stopped(2) → inactive(0)
    execute "UPDATE rules SET status = 0 WHERE status = 2"

    remove_column :rules, :settings_changed_at
  end
end
