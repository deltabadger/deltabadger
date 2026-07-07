# Shared "add the exchange API key" wizard step. `new` skips ahead when a key
# is already confirmed correct; `create` validates the submitted credentials and
# branches on the outcome. Subclasses supply the bot relation and routes as
# explicit overrides.
class Bots::Wizard::AddApiKeysController < ApplicationController
  before_action :authenticate_user!

  def new
    @bot = build_bot

    if (path = prerequisite_redirect_path)
      redirect_to path
    elsif @bot.exchange_id.blank?
      redirect_to missing_exchange_path
    else
      @api_key = @bot.api_key
      # Only validate if key exists but isn't already confirmed correct
      if @api_key.key.present? && @api_key.secret.present? && !@api_key.correct?
        result = @api_key.get_validity
        @api_key.update_status!(result)
      end
      redirect_to after_api_key_path if @api_key.correct?
    end
  end

  def create
    @bot = build_bot
    if @bot.exchange_id.blank?
      redirect_to missing_exchange_path
      return
    end
    @api_key = @bot.api_key
    @api_key.validate_credentials!(api_key_params)
    if @api_key.correct?
      render turbo_stream: turbo_stream_redirect(after_api_key_path)
    elsif @api_key.incorrect?
      flash.now[:alert] = t('errors.incorrect_api_key_permissions')
      render :create, status: :unprocessable_entity
    else
      flash.now[:alert] = t('errors.api_key_permission_validation_failed')
      render :create, status: :unprocessable_entity
    end
  end

  private

  def bot_relation
    raise NotImplementedError
  end

  def build_bot = bot_relation.new(sanitized_bot_config)

  # An earlier-step prerequisite is missing (stale/direct URL) — subclasses
  # return a path to bounce back to; nil means proceed.
  def prerequisite_redirect_path = nil

  def api_key_params
    params.require(:api_key).permit(:key, :secret, :passphrase, :access_token, :rsa_signature_key, :rsa_encryption_key, :dh_param, :ibkr_realm)
  end
end
