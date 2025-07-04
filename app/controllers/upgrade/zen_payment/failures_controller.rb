class Upgrade::ZenPayment::FailuresController < ApplicationController
  before_action :authenticate_user!

  def show
    Rails.logger.error("Zen payment failure: #{params}")
    Raven.capture_exception(Exception.new("Zen payment server error: #{params}"))
    flash[:alert] = t('subscriptions.payment.server_error')
    redirect_to upgrade_path
  end
end
