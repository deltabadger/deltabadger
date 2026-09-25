# frozen_string_literal: true

# The dashboard's Split: `new` is the confirmation modal for the bots the user picked (re-fetched into its
# own frame while a reinvestment settles), `create` splits exactly the bots the modal named. Ids are
# sanitised as Bots::MergesController does: integers, owned by the current user.
class Bots::SplitsController < ApplicationController
  before_action :authenticate_user!

  def new
    @split = Bot::Split.new(current_user, params[:ids])
  end

  def create
    split = Bot::Split.new(current_user, params[:ids], keep_ids: params[:keep_ids])

    if split.perform!
      flash[:notice] = t('bot.split.success')
      render turbo_stream: turbo_stream_redirect(bots_path)
    else
      flash.now[:alert] = split.error
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end
end
