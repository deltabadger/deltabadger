# Single source of the wizard's default settings for a freshly-created
# (unstarted) bot — the settings the wizard never asks about; the user edits
# them on the bot's show page. Consumed by the pick_spendable_assets step
# (which finalises single/dual/index bots) and by the legacy index
# confirm_settings step, so the two can't drift apart.
module Bots::WizardDefaults
  SINGLE = { 'quote_amount' => 100, 'interval' => 'week' }.freeze
  DUAL = SINGLE.merge('allocation0' => 0.5).freeze
  # num_coins default is owned by the model (index-aware: a bounded index starts at full size).
  INDEX = SINGLE.merge('allocation_flattening' => 0.0).freeze

  # Fill only the keys the user hasn't already set.
  def self.apply!(settings, defaults)
    defaults.each { |key, value| settings[key] ||= value }
    settings
  end
end
