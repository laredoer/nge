import { mount, Text, View } from "nsh";

function App() {
  return (
    <View class="flex-1 bg-black">
      <Text class="text-white p-4">关卡编辑器</Text>
    </View>
  );
}

mount(App);
