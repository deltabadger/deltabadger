class Bots::StartsController < ApplicationController
  include Bots::Botable

  before_action :authenticate_user!
  before_action :set_bot

  # The restart question — or, while the wash-sale question is still owed, that one instead. Both
  # land in the same modal frame, and answering is what makes this one reachable.
  #
  # Nothing to ask of a bot that is not restarting: the question is "carry the buy your stop went
  # through, or skip to the next checkpoint", and both halves are an interval bot's. A schedule-less
  # bot (Bots::Signal) has neither — it has no Bot::Lifecycle at all — so the template used to 500
  # on the first thing it asked. An empty frame, the same answer bots/wash_sale_prompts/new gives a
  # stale page whose question is no longer due.
  def edit
    return render partial: 'bots/wash_sale_prompts/dialog', locals: { bot: @bot } if wash_sale_prompt_due?(@bot)

    render html: helpers.turbo_frame_tag('modal') unless @bot.restarting?
  end

  # Starting a bot that can sell is the loudest "I am about to trade" there is, so it is where the
  # question is finally owed rather than merely offered: refused until it is answered, with the
  # question itself as the answer. 422 keeps the modal up, and the stream reaches the bots index
  # too, which has the layout's modal frame and no start template of its own.
  def update
    if wash_sale_prompt_due?(@bot)
      flash.now[:alert] = t('engine.wash_sale_question_owed', label: @bot.label)
      return render turbo_stream: [wash_sale_prompt_stream(@bot), turbo_stream_prepend_flash], status: :unprocessable_entity
    end

    fresh = BotApi::Bots::Start.start_fresh(params[:start_fresh])
    return refuse(t('engine.invalid_start_fresh')) if fresh.nil?
    return if @bot.start(start_fresh: fresh)

    refuse(@bot.errors.messages.values.flatten.to_sentence)
  end

  private

  def refuse(message)
    flash.now[:alert] = message
    render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
  end
end
