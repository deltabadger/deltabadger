class Bots::ArchivesController < ApplicationController
  include Bots::Botable

  before_action :authenticate_user!
  before_action :set_bot

  def edit; end

  def create
    return render_error unless @bot.archive

    render :update
  end

  # Reactivating is the one transition that can move the bot off the page it was clicked on: from
  # ?filter=archived it leaves the list (and may take the filter with it). Refresh rather than
  # patch widgets into a tile that no longer belongs there.
  def destroy
    return render_error unless @bot.unarchive

    render turbo_stream: turbo_stream_page_refresh
  end

  private

  def render_error
    flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
    render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
  end
end
