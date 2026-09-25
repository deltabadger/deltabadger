module Automation::Configurable
  extend ActiveSupport::Concern

  included do
    before_save :update_settings_changed_at, if: :will_save_change_to_settings?
  end

  # A stored row can lack a settings key that an after_initialize concern defaults, and the concern
  # fills it in memory on every load. That fill is not a settings change: counting it would move
  # settings_changed_at and trip Bot::Accountable's guard on saves that never touched settings (a
  # delete, a fresh start). Only what differs from the row as loaded counts. The fill still reaches
  # the row with the next save; dirty tracking is left alone, so settings_in_database stays the row.
  # A filled key counts only while the row still lacks it: once any write (update_columns included)
  # has stored a value, that value is the baseline again.
  def settings_changed_since_load?
    stored = settings_was.to_h
    settings != stored.merge(@settings_filled_on_load.to_h.select { |key, _| stored[key].nil? })
  end

  # Runs once per record read from the database, after every find/initialize callback, whatever order
  # the concerns were included in. Public, as in ActiveRecord::Core: Rails calls it with a receiver.
  def init_with_attributes(...)
    super.tap { remember_settings_filled_on_load unless new_record? }
  end

  # reload takes a fresh load's attributes, fill included, without passing through the above.
  def reload(...)
    super.tap { remember_settings_filled_on_load }
  end

  private

  def remember_settings_filled_on_load
    stored = settings_in_database.to_h
    # deep_dup: an in-place edit of a filled value (String#replace) must not edit the record of it.
    @settings_filled_on_load = settings.to_h.select { |key, _| stored[key].nil? }.deep_dup
  end

  def update_settings_changed_at
    return unless settings_changed_since_load?

    self.settings_changed_at = Time.current
  end
end
