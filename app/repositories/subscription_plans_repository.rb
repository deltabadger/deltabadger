class SubscriptionPlansRepository < BaseRepository
  def model
    SubscriptionPlan
  end

  def saver
    find_by_name!(model::SAVER)
  end

  def investor
    find_by_name!(model::INVESTOR)
  end

  def hodler
    find_by_name!(model::HODLER)
  end

  def upgrade?(old_plan, new_plan)
    plans = [model::SAVER, model::INVESTOR, model::HODLER]
    old_plan_index = plans.index(old_plan.name)
    new_plan_index = plans.index(new_plan.name)

    new_plan_index > old_plan_index
  end

  private

  def plan_cache
    @plan_cache ||= model.all.map { |sp| [sp.name, sp] }.to_h
  end

  def find_by_name!(name)
    plan_cache.fetch(name)
  end
end
