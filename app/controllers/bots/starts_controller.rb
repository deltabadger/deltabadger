class Bots::StartsController < ApplicationController
  include Bots::Botable

  before_action :authenticate_user!
  before_action :set_bot

  # The restart question — or, while the wash-sale question is still owed, that one instead. Both
  # land in the same modal frame, and answering is what makes this one reachable.
  def edit
    render partial: 'bots/wash_sale_prompts/dialog', locals: { bot: @bot } if wash_sale_prompt_due?(@bot)
  end

  # Starting a bot that can sell is the loudest "I am about to trade" there is, so it is where the
  # question is finally owed rather than merely offered: refused until it is answered, with the
  # question itself as the answer. 422 keeps the modal up, and the stream reaches the bots index
  # too, which has the layout's modal frame and no start template of its own.
  def update
    return render turbo_stream: wash_sale_prompt_stream(@bot), status: :unprocessable_entity if wash_sale_prompt_due?(@bot)
    return if @bot.start(start_fresh: Utilities::String.to_boolean(params[:start_fresh]))

    flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
    render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
  end
end
