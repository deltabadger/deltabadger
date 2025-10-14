class Surveys::OnboardingV2sController < ApplicationController
  before_action :authenticate_user!
  skip_before_action :check_onboarding_survey
  before_action :check_onboarding_status

  layout 'devise'

  def new_step_one
    @survey = current_user.surveys.onboarding_v2.new
    @investment_methods = [
      { id: 'exchange', name: t('onboarding_v2.survey.step1.options.exchange') },
      { id: 'defi', name: t('onboarding_v2.survey.step1.options.defi') }
    ].shuffle
  end

  def new_step_two
    return redirect_to step_one_surveys_onboarding_v2_path unless survey_params[:investment_method].present?

    @survey = current_user.surveys.onboarding_v2.new(answers: { investment_method: survey_params[:investment_method] })
    @investment_assets = [
      { id: 'bitcoin', name: t('onboarding_v2.survey.step2.options.bitcoin') },
      { id: 'ethereum', name: t('onboarding_v2.survey.step2.options.ethereum') },
      { id: 'top_5_crypto', name: t('onboarding_v2.survey.step2.options.top_5_crypto') },
      { id: 'top_10_crypto', name: t('onboarding_v2.survey.step2.options.top_10_crypto') },
      { id: 'top_50_crypto', name: t('onboarding_v2.survey.step2.options.top_50_crypto') },
      { id: 'altcoin_season', name: t('onboarding_v2.survey.step2.options.altcoin_season') },
      { id: 'magnificent_7', name: t('onboarding_v2.survey.step2.options.magnificent_7') },
      { id: 'nasdaq_100', name: t('onboarding_v2.survey.step2.options.nasdaq_100') },
      { id: 'sp_500', name: t('onboarding_v2.survey.step2.options.sp_500') },
      { id: 'gold', name: t('onboarding_v2.survey.step2.options.gold') },
      { id: 'other_stocks', name: t('onboarding_v2.survey.step2.options.other_stocks') },
      { id: 'other_crypto', name: t('onboarding_v2.survey.step2.options.other_crypto') }
    ].shuffle
  end

  def create
    @survey = current_user.surveys.onboarding_v2.new(answers: survey_params)
    if @survey.save
      redirect_to root_path
    else
      redirect_to step_one_surveys_onboarding_v2_path
    end
  end

  private

  def check_onboarding_status
    redirect_to root_path if current_user.surveys.onboarding_v2.exists?
  end

  def survey_params
    params.require(:surveys_onboarding_v2).permit(:investment_method, investment_assets: [])
  end
end
