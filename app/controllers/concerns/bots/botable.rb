module Bots::Botable
  extend ActiveSupport::Concern

  private

  def set_bot
    @bot = current_user.bots.find(params[:bot_id] || params[:id])
    redirect_to bots_path, alert: t('bot.not_found') if @bot.deleted?
  rescue ActiveRecord::RecordNotFound
    redirect_to bots_path, alert: t('bot.not_found')
  end

  # For writes that answer only a Turbo stream: any other format is refused (406) before it writes,
  # instead of committing and then finding no template to render.
  def require_turbo_stream
    raise ActionController::UnknownFormat unless request.format.turbo_stream?
  end
end
