# Shared top-level create for the four bot types: build the bot from the wizard
# session, start it fresh, clear the session, and break out of the modal to the
# bot's show page. Subclasses supply the bot relation and type-specific build
# steps via explicit overrides — nothing is derived from class names.
class Bots::Wizard::CreatesController < ApplicationController
  before_action :authenticate_user!

  def create
    @bot = build_bot
    prepare_bot_for_save(@bot)
    if @bot.save && @bot.start(start_fresh: true)
      session[:bot_config] = nil
      render turbo_stream: turbo_stream_redirect(bot_path(@bot))
    else
      flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  private

  def bot_relation
    raise NotImplementedError
  end

  def build_bot
    bot_relation.new(sanitized_bot_config.deep_symbolize_keys)
  end

  # Accountable types must set missed_quote_amount before settings-changing saves.
  def prepare_bot_for_save(bot)
    bot.set_missed_quote_amount
  end
end
