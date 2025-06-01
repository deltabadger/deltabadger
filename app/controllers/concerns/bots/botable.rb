module Bots::Botable
  extend ActiveSupport::Concern

  private

  def set_bot
    @bot = current_user.bots.find(params[:id] || params[:bot_id])
    redirect_to bots_path, alert: t('bot.not_found') if @bot.deleted?
  rescue ActiveRecord::RecordNotFound
    redirect_to bots_path, alert: t('bot.not_found')
  end
end
