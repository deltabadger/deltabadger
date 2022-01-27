module Admin
  class SettingsController < Admin::ApplicationController
    before_action :authenticate_user!
    protect_from_forgery except: %(change_setting)
    def change_setting
      setting = Setting.find_by(name: params['name'])
      return if setting.nil?

      setting.update(value: params['value'].to_s)
    end
  end
end
