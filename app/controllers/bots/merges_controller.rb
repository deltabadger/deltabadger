# frozen_string_literal: true

# The dashboard's Merge: `new` is the confirmation modal for the bots the user picked, `create` merges
# exactly the bots the modal named. Ids are sanitised the way the reorder endpoint does it — integers,
# owned by the current user — with no strong-params ceremony; ownership is the filter.
class Bots::MergesController < ApplicationController
  before_action :authenticate_user!

  def new
    @merge = Bot::Merge.new(current_user, params[:ids], exchange_id:)
  end

  def create
    merge = Bot::Merge.new(current_user, params[:ids], exchange_id:)
    merged = merge.perform!

    if merged
      flash[:notice] = t('bot.merge.success')
      render turbo_stream: turbo_stream_redirect(bot_path(merged))
    else
      flash.now[:alert] = merge.error
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  private

  # The venue picked in the modal; absent until the user picks one.
  def exchange_id = params[:exchange_id].presence&.to_i
end
