class ArticlesController < ApplicationController
  include Pagy::Backend

  before_action :set_premium_subscribers_count, only: %i[index show]
  before_action :set_article, only: [:show]

  def index
    @pagy, @articles = pagy(Article.published.by_locale(I18n.locale).recent, limit: 10)
  end

  def show
    return redirect_to articles_path unless @article&.published?

    @content = @article.render_content(user: current_user)
    @content_unlocked = current_user&.can_access_full_articles?
  end

  private

  def set_article
    @article = Article.published.by_locale(I18n.locale).find_by(slug: params[:id])
    return if @article.present?

    # Try to find in other locales and redirect
    article_in_other_locale = Article.published.find_by(slug: params[:id])
    if article_in_other_locale.present?
      redirect_to article_path(article_in_other_locale, locale: article_in_other_locale.locale)
    else
      redirect_to articles_path
    end
  end

  def set_premium_subscribers_count
    @premium_subscribers_count = SubscriptionPlan.pro.active_subscriptions_count +
                                 SubscriptionPlan.legendary.active_subscriptions_count
  end
end
