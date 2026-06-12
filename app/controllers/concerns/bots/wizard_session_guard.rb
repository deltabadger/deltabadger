module Bots::WizardSessionGuard
  extend ActiveSupport::Concern

  private

  # Mid-wizard create with an expired session (cookie expiry or hand-crafted
  # request) bails to root rather than crashing on the missing config.
  def redirect_if_session_expired
    render turbo_stream: turbo_stream_redirect(root_path) if session[:bot_config].blank?
  end
end
