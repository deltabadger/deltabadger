module Automation::Configurable
  extend ActiveSupport::Concern

  included do
    before_save :update_settings_changed_at, if: :will_save_change_to_settings?
  end

  private

  def update_settings_changed_at
    # FIXME: Required because we are using store_accessor and will_save_change_to_settings?
    # always returns true, at least in Rails 6.0
    return if settings_was == settings

    self.settings_changed_at = Time.current
  end
end
