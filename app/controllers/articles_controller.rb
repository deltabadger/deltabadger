class ArticlesController < ApplicationController
  before_action :set_article, only: [:show]

  def index
    @articles = Article.published
                       .by_locale(I18n.locale)
                       .recent
                       .page(params[:page])
                       .per(10)

    @page_title = t('articles.index.title')
    @meta_description = t('articles.index.meta_description')
    @premium_subscribers_count = SubscriptionPlan.pro.active_subscriptions_count +
                                 SubscriptionPlan.legendary.active_subscriptions_count
  end

  def show
    return redirect_to articles_path unless @article&.published?

    @content = @article.render_content(user: current_user)
    @has_paywall = @article.has_paywall?
    @user_has_access = @article.user_has_access?(current_user)
    @available_locales = @article.available_locales

    @page_title = @article.title
    @meta_description = @article.render_excerpt
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
end
