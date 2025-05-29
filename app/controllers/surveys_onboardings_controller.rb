class SurveysOnboardingsController < ApplicationController
  before_action :authenticate_user!
  skip_before_action :check_onboarding_survey
  before_action :check_onboarding_status

  layout 'devise'

  def new_step_one
    @survey = current_user.surveys.onboarding.new
    @investment_goals = [
      { id: 'buy_the_dip', name: t('onboarding.survey.step1.options.buy_the_dip').html_safe },
      { id: 'retire_early', name: t('onboarding.survey.step1.options.retire_early').html_safe }
    ].shuffle
  end

  def new_step_two
    return redirect_to step_one_surveys_onboarding_path unless survey_params[:investment_goal].present?

    @survey = current_user.surveys.onboarding.new(answers: { investment_goal: survey_params[:investment_goal] })
    @exchanges = [
      { id: 'binance', name: 'Binance' },
      { id: 'binance_us', name: 'Binance.US' },
      { id: 'bingx', name: 'BingX' },
      { id: 'bitfinex', name: 'Bitfinex' },
      { id: 'bitget', name: 'Bitget' },
      { id: 'bitpanda', name: 'Bitpanda' },
      { id: 'bitso', name: 'Bitso' },
      { id: 'bitstamp', name: 'Bitstamp' },
      { id: 'bitvavo', name: 'Bitvavo' },
      { id: 'btse', name: 'BTSE' },
      { id: 'bybit', name: 'Bybit' },
      { id: 'cexio', name: 'CEX.IO' },
      { id: 'coinbase', name: 'Coinbase' },
      { id: 'deribit', name: 'Deribit' },
      { id: 'gateio', name: 'Gate' },
      { id: 'gemini', name: 'Gemini' },
      { id: 'kraken', name: 'Kraken' },
      { id: 'kucoin', name: 'KuCoin' },
      { id: 'mexc', name: 'MEXC' },
      { id: 'okx', name: 'OKX' },
      { id: 'probit', name: 'Probit Global' },
      { id: 'whitebit', name: 'WhiteBit' },
      { id: 'zondacrypto', name: 'Zondacrypto' }
    ].shuffle
    @exchanges << { id: 'other', name: t('onboarding.survey.step2.other') }
  end

  def create
    @survey = current_user.surveys.onboarding.new(answers: survey_params)
    if @survey.save
      redirect_to bots_path
    else
      redirect_to step_one_surveys_onboarding_path
    end
  end

  private

  def check_onboarding_status
    redirect_to bots_path if current_user.surveys.onboarding.exists?
  end

  def survey_params
    params.require(:surveys_onboarding).permit(:investment_goal, preferred_exchange: [])
  end
end
