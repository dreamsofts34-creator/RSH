namespace ApexLink.App.Views;

/// <summary>Jo page har UI tick par taaza hota hai (MainWindow ka 30 Hz timer bulata hai).</summary>
public interface IUpdatableView
{
    void Tick();
}
