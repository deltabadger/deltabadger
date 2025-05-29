module Botable
  extend ActiveSupport::Concern

  included do
    before_action :set_bot, only: %i[create show edit update destroy]
  end

  private

  def set_bot
    @bot = current_user.bots.find(params[:id] || params[:bot_id])
  rescue ActiveRecord::RecordNotFound
    redirect_to bots_path, alert: t('bot.not_found')
  end
end
