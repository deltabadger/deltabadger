module Admin
  class SettingsController < Admin::ApplicationController
    before_action :authenticate_user!
    skip_before_action :verify_authenticity_token

    def change_setting_flag
      setting = SettingFlag.find_by(name: params['name'])
      return if setting.nil?

      setting.update(value: params['value'])
    end
  end
end
